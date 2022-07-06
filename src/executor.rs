use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    hash::Hash,
    sync::{Arc, Mutex},
};

use futures::channel::mpsc::{channel, Receiver, Sender};
use futures::{SinkExt, StreamExt};
use futures_any::stream_sink::{AnySink, AnyStream};

use crate::{
    channel::Channel,
    multiplexing::{MultiplexerIncoming, MultiplexerOutgoing},
};

pub struct Executor<Mux, Id, Output, Input, Error>
where
    Id: Display + Clone + 'static,
    Output: Debug + 'static,
    Input: Debug + 'static,
    Error: From<std::io::Error> + 'static,
    Mux: MultiplexerIncoming<Input, Error = Error, Id = Id>
        + MultiplexerOutgoing<Output, Error = Error, Id = Id>
        + 'static
        + Send,
{
    stream_senders: Arc<Mutex<HashMap<Id, Sender<Result<Mux::Input, Error>>>>>,
    mux: Arc<Mutex<Mux>>,
    stream: AnyStream<Result<Input, Error>>,
    sink: AnySink<Mux::MuxOutput, Error>,
    channel_sender: Option<Sender<Channel<Id, Output, Mux::Input, Error>>>,
    channel_receiver: Option<Receiver<Channel<Id, Output, Mux::Input, Error>>>,
    sink_sender: Sender<(Id, Output)>,
    sink_receiver: Option<Receiver<(Id, Output)>>,
    buffer: usize,
}

impl<Mux, Id, Output, Input, Error> Executor<Mux, Id, Output, Input, Error>
where
    Id: Display + Clone + Eq + Hash + Send,
    Output: Debug + Send,
    Input: Debug + Send,
    Error: From<std::io::Error> + Display + Send,
    Mux: MultiplexerIncoming<Input, Error = Error, Id = Id>
        + MultiplexerOutgoing<Output, Error = Error, Id = Id>
        + Send
        + 'static,
    Mux::Input: Send + 'static,
{
    pub fn new(
        mux: Mux,
        stream: AnyStream<Result<Input, Error>>,
        sink: AnySink<Mux::MuxOutput, Error>,
        buffer: usize,
    ) -> Self {
        let (channel_sender, channel_receiver) = channel(buffer);

        let (sink_sender, sink_receiver) = channel(buffer);

        Self {
            stream_senders: Default::default(),
            mux: Arc::new(Mutex::new(mux)),
            stream,
            sink,
            channel_sender: Some(channel_sender),
            channel_receiver: Some(channel_receiver),
            sink_sender,
            sink_receiver: Some(sink_receiver),
            buffer,
        }
    }

    pub async fn join(&mut self) -> Result<(), Error> {
        use futures::future::Either;

        loop {
            let mut sink_receiver = self.sink_receiver.take().expect("call join twice");

            let sink_fut = sink_receiver.next();

            let selector = futures::future::select(sink_fut, self.stream.next()).await;

            match selector {
                Either::Left((left, _)) => {
                    self.sink_receiver = Some(sink_receiver);
                    match left {
                        Some((id, output)) => {
                            self.dispatch_outgoing(id, output).await?;
                        }
                        None => return Ok(()),
                    }
                }
                Either::Right((right, _)) => {
                    self.sink_receiver = Some(sink_receiver);

                    match right {
                        Some(Ok(input)) => {
                            self.dipsatch_incoming(input).await?;
                        }
                        Some(Err(err)) => {
                            return Err(err);
                        }
                        None => return Ok(()),
                    }
                }
            };
        }
    }

    async fn dispatch_outgoing(&mut self, id: Id, output: Output) -> Result<(), Error> {
        let result = { self.mux.lock().unwrap().outgoing(output, id) };
        match result {
            Err(err) => {
                // close sink channel
                drop(
                    self.sink_receiver
                        .take()
                        .expect("current call dispatch_outgoing"),
                );

                Err(err)
            }
            Ok(item) => match self.sink.send(item).await {
                Err(err) => {
                    // close sink channel
                    drop(
                        self.sink_receiver
                            .take()
                            .expect("current call dispatch_outgoing"),
                    );

                    Err(err)
                }
                Ok(_) => Ok(()),
            },
        }
    }

    async fn dipsatch_incoming(&mut self, input: Input) -> Result<(), Error> {
        use std::io;

        let result = { self.mux.lock().unwrap().incoming(input) };

        match result {
            Err(err) => {
                let senders = self.stream_senders.lock().unwrap().clone();

                for (_, mut sender) in senders {
                    // skip check send result

                    _ = sender
                        .send(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            anyhow::format_err!("{}", err),
                        )
                        .into()))
                        .await;
                }

                // close all channel
                self.stream_senders.lock().unwrap().clear();

                return Err(err);
            }
            Ok((id, item, close_channel)) => {
                let mut sender = self.stream_senders.lock().unwrap().remove(&id);

                if sender.is_none() {
                    let (s, r) = channel(self.buffer);

                    sender = Some(s);

                    log::trace!("new connect {}", id);

                    let channel_id = id.clone();

                    let mut channel_sender =
                        self.channel_sender.take().expect("call exeuctor twice");

                    let channel_stream_senders = self.stream_senders.clone();

                    let channel_stream_mux = self.mux.clone();

                    channel_sender
                        .send(Channel::new(
                            id.clone(),
                            r,
                            self.sink_sender.clone(),
                            move || {
                                log::debug!("call disconnect func {}", channel_id.to_string());
                                channel_stream_senders.lock().unwrap().remove(&channel_id);
                                channel_stream_mux.lock().unwrap().disconnect(channel_id);
                            },
                        ))
                        .await
                        .map_err(|err| {
                            std::io::Error::new(std::io::ErrorKind::BrokenPipe, err).into()
                        })?; // no error promise

                    self.channel_sender = Some(channel_sender);
                }

                let mut sender = sender.unwrap();

                match sender.send(Ok(item)).await {
                    // channel remote endpoint is closed
                    Err(_) => {
                        if !close_channel {
                            self.mux.lock().unwrap().disconnect(id);
                        }
                    }
                    _ => {
                        if !close_channel {
                            self.stream_senders.lock().unwrap().insert(id, sender);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// This funs can only be called once.
    pub fn incoming(&mut self) -> Receiver<Channel<Id, Output, Mux::Input, Error>> {
        self.channel_receiver.take().expect("call incoming twice")
    }

    /// Reentry function, each call returns a new closure.
    /// Highly recommended, the function is called only once by the client
    pub fn connector(
        &self,
    ) -> impl FnMut() -> Result<Channel<Id, Output, Mux::Input, Error>, Error> + 'static {
        let sink_sender = self.sink_sender.clone();

        let mux = self.mux.clone();

        let channel_stream_senders = self.stream_senders.clone();

        let channel_stream_mux = self.mux.clone();

        let buffer = self.buffer;

        move || match mux.lock().unwrap().connect() {
            Ok(id) => {
                let (s, r) = channel(buffer);

                channel_stream_senders.lock().unwrap().insert(id.clone(), s);

                let channel_stream_senders = channel_stream_senders.clone();

                let channel_stream_mux = channel_stream_mux.clone();

                Ok(Channel::new(
                    id.clone(),
                    r,
                    sink_sender.clone(),
                    move || {
                        log::debug!("call disconnect func {}", id);
                        channel_stream_senders.lock().unwrap().remove(&id);
                        channel_stream_mux.lock().unwrap().disconnect(id);
                    },
                ))
            }
            Err(err) => Err(err),
        }
    }

    // Get executor connection accounter
    pub fn connection_accounter(&self) -> impl FnMut() -> usize {
        let channel_stream_senders = self.stream_senders.clone();

        move || channel_stream_senders.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    struct NullMultiplexer(u32);

    impl MultiplexerIncoming<u32> for NullMultiplexer {
        type Error = TestError;
        type Id = u32;
        type Input = u32;

        fn incoming(&mut self, data: u32) -> Result<(Self::Id, Self::Input, bool), Self::Error> {
            log::trace!("incoming .... {}", data);
            Ok((self.0, data, false))
        }

        fn disconnect(&mut self, _: Self::Id) {
            self.0 += 1;
        }
    }

    impl MultiplexerOutgoing<u32> for NullMultiplexer {
        type Error = TestError;
        type MuxOutput = u32;
        type Id = u32;

        fn outgoing(&mut self, data: u32, _id: u32) -> Result<Self::MuxOutput, Self::Error> {
            Ok(data)
        }

        fn connect(&mut self) -> Result<u32, Self::Error> {
            self.0 += 1;
            Ok(self.0)
        }
    }

    use futures::TryStreamExt;
    use futures_any::stream_sink::{AnySinkEx, AnyStreamEx};
    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum TestError {
        #[error("read/write failed: {0}")]
        IOFailed(String),
    }

    impl From<std::io::Error> for TestError {
        fn from(err: std::io::Error) -> Self {
            TestError::IOFailed(format!("{}", err))
        }
    }

    #[async_std::test]
    async fn test_executor() -> Result<(), anyhow::Error> {
        let (mut incoming_sender, incoming_receiver) = channel::<Result<u32, TestError>>(100);

        let (outgoing_sender, mut outgoing_receiver) = channel::<u32>(100);

        let mut executor = Executor::new(
            NullMultiplexer(0),
            incoming_receiver.to_any_stream(),
            outgoing_sender
                .sink_map_err(|err| -> TestError {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, err).into()
                })
                .to_any_sink(),
            10,
        );

        let mut incoming = executor.incoming();

        let mut connector = executor.connector();

        let mut accounter = executor.connection_accounter();

        async_std::task::spawn(async move {
            executor.join().await?;

            Ok::<(), anyhow::Error>(())
        });

        for i in 1..100 {
            // connection raii test
            assert_eq!(accounter(), 0);

            incoming_sender.send(Ok(i)).await?;

            let connection = incoming.next().await;

            assert!(connection.is_some(), "incoming connection is some");

            let mut connection = connection.unwrap();

            assert_eq!(connection.try_next().await?, Some(i));

            connection.send(i).await?;

            let mut connection = connector()?;

            connection.send(i).await?;

            assert_eq!(outgoing_receiver.next().await, Some(i));

            assert_eq!(outgoing_receiver.next().await, Some(i));

            assert_eq!(accounter(), 2);
        }

        Ok(())
    }
}
