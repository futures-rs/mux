pub mod multiplexer;
pub mod sink;
pub mod stream;

use std::{
    collections::HashMap,
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    future::Either,
    Sink, SinkExt, Stream, StreamExt,
};
use futures_any::stream_sink::{AnySink, AnyStream, AnyStreamEx};
use multiplexer::{MultiplexerIncoming, MultiplexerOutgoing};
use sink::MuxSink;
use stream::MuxStream;

pub struct MultiplexerLoop<Id, Output, Input> {
    stream: AnyStream<Result<(Id, Option<Input>), anyhow::Error>>,
    dispatcher: Arc<Mutex<HashMap<Id, Sender<Input>>>>,
    on_connect: Sender<(Id, Receiver<Input>)>,
    on_disconnect: Box<dyn FnMut(Id) + Send>,
    cached: usize,
    sink: AnySink<(Id, Output), anyhow::Error>,
    receiver: Receiver<(Id, Output)>,
}

impl<Id, Output, Input> MultiplexerLoop<Id, Output, Input>
where
    Id: Eq + Hash + Clone + Debug,
    Input: Debug,
{
    pub fn new(
        stream: AnyStream<Result<(Id, Option<Input>), anyhow::Error>>,
        sink: AnySink<(Id, Output), anyhow::Error>,
        receiver: Receiver<(Id, Output)>,
        on_connect: Sender<(Id, Receiver<Input>)>,
        on_disconnect: Box<dyn FnMut(Id) + Send>,
        cached: usize,
    ) -> Self {
        MultiplexerLoop {
            stream,
            dispatcher: Default::default(),
            on_connect,
            on_disconnect,
            cached,
            receiver,
            sink,
        }
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        loop {
            match futures::future::select(self.receiver.next(), self.stream.next()).await {
                Either::Left((left, _)) => match left {
                    Some((id, output)) => {
                        self.dispatch_outgoing(id, output).await?;
                    }
                    None => return Ok(()),
                },
                Either::Right((right, _)) => match right {
                    Some(Ok((id, input))) => {
                        self.dipsatch_incoming(id, input).await?;
                    }
                    Some(Err(err)) => {
                        return Err(err);
                    }
                    None => return Ok(()),
                },
            };
        }
    }

    async fn dispatch_outgoing(&mut self, id: Id, output: Output) -> Result<(), anyhow::Error> {
        match self.sink.send((id.clone(), output)).await {
            Err(err) => {
                log::error!("{:?}", err);
                let disconnect = &mut self.on_disconnect;
                disconnect(id);
            }
            _ => {}
        };

        Ok(())
    }

    async fn dipsatch_incoming(
        &mut self,
        id: Id,
        input: Option<Input>,
    ) -> Result<(), anyhow::Error> {
        log::trace!("{:?} {:?}", id, input);
        let mut sender = self.dispatcher.lock().unwrap().remove(&id);

        if input.is_none() && sender.is_some() {
            return Ok(());
        }

        if sender.is_none() {
            let (s, r) = channel::<Input>(self.cached);

            sender = Some(s);

            log::trace!("new connect {:?}", id);

            self.on_connect.send((id.clone(), r)).await?;
        }

        let mut sender = sender.unwrap();

        match sender.send(input.unwrap()).await {
            Err(err) => {
                log::trace!("disconnect {:?}", id);
                if err.is_disconnected() {
                    let f = &mut self.on_disconnect;
                    f(id);
                    return Ok(());
                }
            }
            _ => {}
        }

        self.dispatcher.lock().unwrap().insert(id, sender);
        Ok(())
    }
}

pub struct MultiplexerChannel<Id, Output, Input> {
    pub id: Id,
    pub stream: Receiver<Input>,
    pub sink: Sender<(Id, Output)>,
    pub disconnect: Box<dyn FnMut(Id) + Send>,
    _marker: PhantomData<Output>,
}

impl<Id, Output, Input> MultiplexerChannel<Id, Output, Input>
where
    Id: Clone,
{
    pub fn disconnect(&mut self) {
        let disconnect = &mut self.disconnect;

        disconnect(self.id.clone());
    }

    pub async fn next(&mut self) -> Option<Input> {
        self.stream.next().await
    }

    pub async fn send(&mut self, output: Output) -> Result<(), anyhow::Error> {
        self.sink
            .send((self.id.clone(), output))
            .await
            .map_err(|err| anyhow::Error::new(err))
    }
}

pub type Connector<Id, Output, Input> =
    Box<dyn FnMut() -> Result<MultiplexerChannel<Id, Output, Input>, anyhow::Error>>;

pub struct Multiplexer<Id, Output, Input> {
    pub event_loop: MultiplexerLoop<Id, Output, Input>,
    pub incoming: AnyStream<MultiplexerChannel<Id, Output, Input>>,
    pub connect: Connector<Id, Output, Input>,
}

impl<Id, Output, Input> Multiplexer<Id, Output, Input> {
    pub fn new<R, W, MX, Error>(r: R, w: W, mx: MX, cached: usize) -> Self
    where
        R: Stream + Unpin,
        W: Sink<<MX as MultiplexerOutgoing<Output>>::MuxOutput> + Unpin,
        Error: std::error::Error + Send + Sync + 'static,
        MX: MultiplexerOutgoing<Output, Id = Id, Error = Error>
            + MultiplexerIncoming<R::Item, Error = Error, Input = Input, Id = Id>
            + Send,
        W::Error: std::error::Error + Send + Sync + 'static,
        Id: Eq + Hash + Clone + 'static + Debug,
        Input: 'static + Debug,
        Output: 'static,
        MX: 'static,
    {
        let mx = Arc::new(Mutex::new(mx));

        let stream = r.mux_incoming(mx.clone());
        let sink = w.mux_outgoing(mx.clone());

        let (on_connct_sender, on_connect_receiver) = channel::<(Id, Receiver<Input>)>(cached);

        let mx_disconnect = mx.clone();

        let on_disconnect = Box::new(move |id| {
            mx_disconnect.lock().unwrap().disconnect(id);
        });

        let (output_sender, output_receiver) = channel::<(Id, Output)>(cached);

        let event_loop = MultiplexerLoop::new(
            stream,
            sink,
            output_receiver,
            on_connct_sender,
            on_disconnect.clone(),
            cached,
        );

        let dispatcher = event_loop.dispatcher.clone();

        let output_sender_connect = output_sender.clone();

        let disconnect = on_disconnect.clone();

        let connect = move || -> Result<MultiplexerChannel<Id, Output, Input>, anyhow::Error> {
            let id = mx.lock().unwrap().connect()?;

            let (input_sender, input_receiver) = channel::<Input>(cached);

            dispatcher.lock().unwrap().insert(id.clone(), input_sender);

            Ok(MultiplexerChannel {
                id,
                sink: output_sender_connect.clone(),
                stream: input_receiver,
                _marker: PhantomData,
                disconnect: disconnect.clone(),
            })
        };

        let disconnect = on_disconnect.clone();

        let incoming = on_connect_receiver
            .map(move |(id, receiver)| {
                let channel = MultiplexerChannel {
                    id: id.clone(),
                    sink: output_sender.clone(),
                    stream: receiver,
                    _marker: PhantomData,
                    disconnect: disconnect.clone(),
                };

                channel
            })
            .to_any_stream();

        Multiplexer {
            event_loop,
            incoming,
            connect: Box::new(connect),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use super::*;

    use async_std::task::spawn;
    use futures_any::stream_sink::AnySinkEx;

    struct NullMultiplexer(u32);

    impl MultiplexerIncoming<String> for NullMultiplexer {
        type Error = std::io::Error;
        type Id = u32;
        type Input = String;

        fn incoming(&mut self, data: String) -> Result<(Self::Id, Self::Input, bool), Self::Error> {
            log::trace!("incoming .... {}", data);
            Ok((self.0, data, false))
        }

        fn disconnect(&mut self, _: Self::Id) {
            self.0 += 1;
        }
    }

    impl MultiplexerOutgoing<String> for NullMultiplexer {
        type Error = std::io::Error;
        type MuxOutput = String;
        type Id = u32;

        fn outgoing(&mut self, data: String, _id: u32) -> Result<Self::MuxOutput, Self::Error> {
            Ok(data)
        }

        fn connect(&mut self) -> Result<u32, Self::Error> {
            self.0 += 1;
            Ok(self.0)
        }
    }

    #[async_std::test]
    async fn test_multiplexer() -> Result<(), anyhow::Error> {
        pretty_env_logger::init();

        let mut i = 0;

        let read = futures::stream::poll_fn(|_| -> Poll<Option<String>> {
            i += 1;
            Poll::Ready(Some(format!("hello {}", i)))
        });

        let write = futures::sink::unfold((), |_, data: String| async move {
            log::trace!("send {}", data);
            Ok::<_, futures::never::Never>(())
        });

        futures::pin_mut!(write);

        let Multiplexer {
            mut event_loop,
            mut incoming,
            ..
        } = Multiplexer::new(
            read.to_any_stream(),
            write.to_any_sink(),
            NullMultiplexer(0),
            2,
        );

        spawn(async move {
            event_loop.run().await?;

            Ok::<(), anyhow::Error>(())
        });

        while let Some(mut channel) = incoming.next().await {
            log::trace!("accept {}", channel.id);
            let handle = spawn(async move {
                while let Some(data) = channel.stream.next().await {
                    log::trace!("recv {} {}", channel.id, data);
                    channel.sink.send((channel.id, "Echo".to_owned())).await?;
                }

                Ok::<(), anyhow::Error>(())
            });

            let result = handle.await;

            log::trace!("accept {} quit {:?}", channel.id, result);
        }

        Ok(())
    }

    #[async_std::test]
    async fn test_connect() -> anyhow::Result<()> {
        pretty_env_logger::init();

        let read = futures::stream::poll_fn(|_| -> Poll<Option<String>> { Poll::Pending });

        let write = futures::sink::unfold((), |_, data: String| async move {
            log::trace!("send {}", data);
            Ok::<_, futures::never::Never>(())
        });

        futures::pin_mut!(write);

        {
            let Multiplexer {
                mut event_loop,
                mut connect,
                ..
            } = Multiplexer::new(read, write, NullMultiplexer(0), 2);

            spawn(async move {
                log::trace!("start event loop");

                let result = event_loop.run().await;

                log::trace!("sender stop {:?}", result);

                Ok::<(), anyhow::Error>(())
            });

            let mut channel = connect()?;

            for i in 0..100 {
                log::trace!("send {}", i);
                channel
                    .sink
                    .send((channel.id, format!("Echo {}", i)))
                    .await?;
            }
        }

        Ok(())
    }
}
