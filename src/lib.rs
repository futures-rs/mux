pub mod multiplexer;
pub mod sink;
pub mod stream;

use std::{
    collections::HashMap,
    fmt::Display,
    hash::Hash,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    Sink, SinkExt, Stream, StreamExt, TryStreamExt,
};
use futures_any::stream_sink::{AnySink, AnyStream, AnyStreamEx};
use multiplexer::{MultiplexerIncoming, MultiplexerOutgoing};
use sink::MuxSink;
use stream::MuxStream;

pub struct MultiplexerReceiver<Id, Input> {
    stream: AnyStream<Result<(Id, Input), anyhow::Error>>,
    dispatcher: Arc<Mutex<HashMap<Id, Sender<Input>>>>,
    on_connect: Sender<(Id, Receiver<Input>)>,
    on_disconnect: Box<dyn FnMut(Id) + Send>,
    cached: usize,
}

impl<Id, Input> MultiplexerReceiver<Id, Input>
where
    Id: Eq + Hash + Clone + Display,
{
    pub fn new(
        stream: AnyStream<Result<(Id, Input), anyhow::Error>>,
        on_connect: Sender<(Id, Receiver<Input>)>,
        on_disconnect: Box<dyn FnMut(Id) + Send>,
        cached: usize,
    ) -> Self {
        MultiplexerReceiver {
            stream,
            dispatcher: Default::default(),
            on_connect,
            on_disconnect,
            cached,
        }
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        while let Some((id, input)) = self.stream.try_next().await? {
            let mut sender = self.dispatcher.lock().unwrap().remove(&id);

            if sender.is_none() {
                let (s, r) = channel::<Input>(self.cached);

                sender = Some(s);

                // log::debug!("new connect {}", id);

                self.on_connect.send((id.clone(), r)).await?;
            }

            let mut sender = sender.unwrap();

            match sender.send(input).await {
                Err(err) => {
                    // log::debug!("disconnect {}", id);
                    if err.is_disconnected() {
                        let f = &mut self.on_disconnect;
                        f(id);
                        continue;
                    }
                }
                _ => {}
            }

            self.dispatcher.lock().unwrap().insert(id, sender);
        }

        Ok(())
    }
}

// use backtrace::Backtrace;

impl<Id, Input> Drop for MultiplexerReceiver<Id, Input> {
    fn drop(&mut self) {
        log::debug!("Drop MultiplexerReceive");
    }
}

pub struct MultiplexerSender<Id, Output> {
    sink: AnySink<(Id, Output), anyhow::Error>,
    receiver: Receiver<(Id, Output)>,
    on_disconnect: Box<dyn FnMut(Id) + Send>,
}

impl<Id, Output> MultiplexerSender<Id, Output>
where
    Id: Eq + Hash + Clone,
{
    pub fn new(
        sink: AnySink<(Id, Output), anyhow::Error>,
        receiver: Receiver<(Id, Output)>,
        on_disconnect: Box<dyn FnMut(Id) + Send>,
    ) -> Self {
        MultiplexerSender {
            sink,
            receiver,
            on_disconnect,
        }
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        log::debug!("MultiplexerSender run...");
        while let Some(output) = self.receiver.next().await {
            let id = output.0.clone();
            match self.sink.send(output).await {
                Err(err) => {
                    log::error!("{:?}", err);
                    let disconnect = &mut self.on_disconnect;
                    disconnect(id);
                }
                _ => {}
            }
        }

        log::debug!("MultiplexerSender stop");

        Ok(())
    }
}

impl<Id, Output> Drop for MultiplexerSender<Id, Output> {
    fn drop(&mut self) {
        log::debug!("Drop MultiplexerSender");
    }
}

#[derive(Debug)]
pub struct MultiplexerChannel<Id, Output, Input> {
    pub id: Id,
    pub stream: Receiver<Input>,
    pub sink: Sender<(Id, Output)>,
    _marker: PhantomData<Output>,
}

pub struct Multiplexer<Id, Output, Input> {
    pub sender: MultiplexerSender<Id, Output>,
    pub receiver: MultiplexerReceiver<Id, Input>,
    pub incoming: AnyStream<MultiplexerChannel<Id, Output, Input>>,
    // pub incoming: Receiver<(Id, Receiver<Input>)>,
    pub connect: Box<dyn FnMut() -> Result<MultiplexerChannel<Id, Output, Input>, anyhow::Error>>,
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
        Id: Eq + Hash + Clone + Display,
        Input: 'static + Display,
        Output: 'static + Display,
        Id: 'static,
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

        let receiver =
            MultiplexerReceiver::new(stream, on_connct_sender, on_disconnect.clone(), cached);

        let (output_sender, output_receiver) = channel::<(Id, Output)>(cached);

        let sender = MultiplexerSender::new(sink, output_receiver, on_disconnect);

        let dispatcher = receiver.dispatcher.clone();

        // let output_sender = Arc::new(output_sender);

        let output_sender_connect = output_sender.clone();

        let connect = move || -> Result<MultiplexerChannel<Id, Output, Input>, anyhow::Error> {
            let id = mx.lock().unwrap().connect()?;

            let (input_sender, input_receiver) = channel::<Input>(cached);

            dispatcher.lock().unwrap().insert(id.clone(), input_sender);

            Ok(MultiplexerChannel {
                id,
                sink: output_sender_connect.clone(),
                stream: input_receiver,
                _marker: PhantomData,
            })
        };

        let incoming = on_connect_receiver
            .map(move |(id, receiver)| {
                log::debug!("new incoming {}", id.clone());

                let channel = MultiplexerChannel {
                    id: id.clone(),
                    sink: output_sender.clone(),
                    stream: receiver,
                    _marker: PhantomData,
                };

                log::debug!("new connnect created {}", id);

                channel
            })
            .to_any_stream();

        Multiplexer {
            sender,
            receiver,
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

    struct NullMultiplexer(u32);

    impl MultiplexerIncoming<String> for NullMultiplexer {
        type Error = std::io::Error;
        type Id = u32;
        type Input = String;

        fn incoming(&mut self, data: String) -> Result<(Self::Id, Self::Input), Self::Error> {
            // log::debug!("incoming .... {}", data);
            Ok((self.0, data))
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
            log::debug!("send {}", data);
            Ok::<_, futures::never::Never>(())
        });

        futures::pin_mut!(write);

        let mx = Multiplexer::new(read, write, NullMultiplexer(0), 2);

        let mut receiver = mx.receiver;
        let mut sender = mx.sender;
        let mut incoming = mx.incoming;

        spawn(async move {
            receiver.run().await?;

            Ok::<(), anyhow::Error>(())
        });

        spawn(async move {
            sender.run().await?;

            log::debug!("sender stop");

            Ok::<(), anyhow::Error>(())
        });

        while let Some(mut channel) = incoming.next().await {
            // log::debug!("accept {}", channel.id);
            let handle = spawn(async move {
                while let Some(data) = channel.stream.next().await {
                    log::debug!("recv {} {}", channel.id, data);
                    channel.sink.send((channel.id, "Echo".to_owned())).await?;
                }

                Ok::<(), anyhow::Error>(())
            });

            let result = handle.await;

            log::debug!("accept {} quit {:?}", channel.id, result);
        }

        Ok(())
    }
}
