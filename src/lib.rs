pub mod multiplexer;

use std::{
    collections::HashMap,
    hash::Hash,
    pin::Pin,
    sync::{Arc, Mutex},
};

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    Sink, SinkExt, Stream, StreamExt,
};
pub use multiplexer::*;

pub struct MultiplexerChannel<Id, Input, Output> {
    id: Id,
    sink_sender: Arc<Mutex<Sender<(Id, Output)>>>,
    receiver: Receiver<Input>,
}

pub struct MultiplexerStream<S, MX, Id, MuxInput>
where
    MX: MultiplexerIncoming<MuxInput>,
{
    stream_senders: Arc<Mutex<HashMap<Id, Sender<MX::Input>>>>,
    stream_accept_sender: Sender<(Id, Receiver<MX::Input>)>,
    inner: S,
    mx: Arc<Mutex<MX>>,
}

impl<S, MX, Id, MuxInput> MultiplexerStream<S, MX, Id, MuxInput>
where
    MX: MultiplexerIncoming<MuxInput, Id = Id>,
    S: Stream<Item = MuxInput> + Unpin,
    MX::Error: std::error::Error + Send + Sync + 'static,
    Id: Eq + Hash + Clone,
{
    fn new(
        inner: S,
        mx: Arc<Mutex<MX>>,
        stream_accept_sender: Sender<(Id, Receiver<MX::Input>)>,
    ) -> Self {
        MultiplexerStream {
            inner,
            mx,
            stream_senders: Default::default(),
            stream_accept_sender,
        }
    }

    async fn on_receive(&mut self, id: Id, item: MX::Input) -> Result<(), anyhow::Error> {
        let mut sender = self.stream_senders.lock().unwrap().remove(&id);

        if sender.is_none() {
            let chan = channel::<MX::Input>(100);

            self.stream_accept_sender.send((id.clone(), chan.1)).await?;

            sender = Some(chan.0);
        }

        let mut sender = sender.unwrap();

        sender.send(item).await?;

        self.stream_senders
            .lock()
            .unwrap()
            .insert(id.clone(), sender);

        Ok(())
    }

    pub async fn on_receive_loop(&mut self) -> Result<(), anyhow::Error> {
        while let Some(input) = self.inner.next().await {
            let (id, input) = self.mx.lock().unwrap().incoming(input)?;

            self.on_receive(id, input).await?;
        }

        Ok(())
    }
}

pub struct MultiplexerSink<S, MX, Id, Output> {
    sink_sender: Arc<Mutex<Sender<(Id, Output)>>>,
    sink_receiver: Receiver<(Id, Output)>,
    inner: S,
    mx: Arc<Mutex<MX>>,
}

impl<S, MX, Id, Output> MultiplexerSink<S, MX, Id, Output>
where
    MX: MultiplexerOutgoing<Output, Id>,
    S: Sink<MX::MuxOutput> + Unpin,
    MX::Error: std::error::Error + Send + Sync + 'static,
    S::Error: std::error::Error + Send + Sync + 'static,
    Output: Send + Sync + 'static,
{
    fn new(inner: S, mx: Arc<Mutex<MX>>) -> Self {
        let (sender, receiver) = channel::<(Id, Output)>(100);

        MultiplexerSink {
            sink_sender: Arc::new(Mutex::new(sender)),
            sink_receiver: receiver,
            inner,
            mx,
        }
    }

    pub async fn on_send_loop(&mut self) -> Result<(), anyhow::Error> {
        while let Some((id, output)) = self.sink_receiver.next().await {
            let output = self.mx.lock().unwrap().outgoing(output, id)?;

            self.inner.send(output).await?;
        }

        Ok(())
    }
}

pub struct Multiplexer<Id, Input, Output> {
    sink_sender: Arc<Mutex<Sender<(Id, Output)>>>,
    stream_accept_receiver: Pin<Box<dyn Stream<Item = MultiplexerChannel<Id, Input, Output>>>>,
    connect: Box<dyn FnMut() -> Result<Id, anyhow::Error> + 'static>,
}

impl<Id, Input, Output> Multiplexer<Id, Input, Output>
where
    Id: Eq + Hash + Clone + Sync + Send + 'static,
{
    pub fn new<R, W, MX, MuxInput, MuxOutput, Error>(
        r: R,
        w: W,
        mx: MX,
    ) -> (
        Self,
        MultiplexerStream<R, MX, Id, MuxInput>,
        MultiplexerSink<W, MX, Id, Output>,
    )
    where
        R: Stream<Item = MuxInput> + Send + Sync + 'static + Unpin,
        W: Sink<MuxOutput> + Send + Sync + 'static + Unpin,
        MX: MultiplexerOutgoing<Output, Id, MuxOutput = MuxOutput, Error = Error>
            + MultiplexerIncoming<MuxInput, Id = Id, Error = Error, Input = Input>
            + Send
            + Sync
            + 'static,
        Error: std::error::Error + Send + Sync + 'static,
        W::Error: std::error::Error + Send + Sync + 'static,
        // MuxInput: Send + Sync + 'static,
        Input: Send + Sync + 'static,
        Output: Send + Sync + 'static,
    {
        let mx = Arc::new(Mutex::new(mx));

        let sink = MultiplexerSink::new(w, mx.clone());

        let (stream_accept_sender, stream_accept_receiver) = channel::<(Id, Receiver<Input>)>(100);

        let stream = MultiplexerStream::new(r, mx.clone(), stream_accept_sender);

        let sink_sender = sink.sink_sender.clone();

        (
            Multiplexer {
                sink_sender: sink_sender.clone(),
                stream_accept_receiver: Box::pin(stream_accept_receiver.map(
                    move |(id, receiver)| MultiplexerChannel {
                        id,
                        sink_sender: sink_sender.clone(),
                        receiver,
                    },
                )),
                connect: Box::new(move || {
                    let id = mx.lock().unwrap().connect()?;

                    Ok::<Id, anyhow::Error>(id)
                }),
            },
            stream,
            sink,
        )
    }

    pub fn incoming(
        &mut self,
    ) -> &mut Pin<Box<dyn Stream<Item = MultiplexerChannel<Id, Input, Output>>>> {
        &mut self.stream_accept_receiver
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
            Ok((self.0, data))
        }
    }

    impl MultiplexerOutgoing<String, u32> for NullMultiplexer {
        type Error = std::io::Error;
        type MuxOutput = String;

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
        let read = futures::stream::poll_fn(|_| -> Poll<Option<String>> {
            Poll::Ready(Some("Hello".to_owned()))
        });

        let write = futures::sink::drain::<String>();

        let (mut mx, mut stream, mut sink) = Multiplexer::new(read, write, NullMultiplexer(0));

        spawn(async move {
            stream.on_receive_loop().await?;

            Ok::<(), anyhow::Error>(())
        });

        spawn(async move {
            sink.on_send_loop().await?;

            Ok::<(), anyhow::Error>(())
        });

        while let Some(channel) = mx.incoming().next().await {}

        Ok(())
    }
}
