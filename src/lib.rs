pub mod multiplexer;
pub mod sink;
pub mod stream;

use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Mutex},
};

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    SinkExt, StreamExt, TryStreamExt,
};
use futures_any::stream_sink::{AnySink, AnyStream};

pub struct MultiplexerReceiver<Id, Input, Error> {
    stream: AnyStream<Result<(Id, Input), Error>>,
    dispatcher: Arc<Mutex<HashMap<Id, Sender<Input>>>>,
    on_connect: Sender<Receiver<Input>>,
}

impl<Id, Input, Error> MultiplexerReceiver<Id, Input, Error>
where
    Error: std::error::Error + Sync + Send + 'static,
    Id: Eq + Hash,
{
    pub fn new(
        stream: AnyStream<Result<(Id, Input), Error>>,
        on_connect: Sender<Receiver<Input>>,
    ) -> Self {
        MultiplexerReceiver {
            stream,
            dispatcher: Default::default(),
            on_connect,
        }
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        while let Some((id, input)) = self.stream.try_next().await? {
            let mut sender = self.dispatcher.lock().unwrap().remove(&id);

            if sender.is_none() {
                let (s, r) = channel::<Input>(1000);

                sender = Some(s);

                self.on_connect.send(r).await?;
            }

            let mut sender = sender.unwrap();

            sender.send(input).await?;

            self.dispatcher.lock().unwrap().insert(id, sender);
        }

        Ok(())
    }
}

pub struct MultiplexerSender<Id, Output, Error> {
    sink: AnySink<(Id, Output), Error>,
    receiver: Receiver<(Id, Output)>,
}

impl<Id, Output, Error> MultiplexerSender<Id, Output, Error>
where
    Error: std::error::Error + Sync + Send + 'static,
    Id: Eq + Hash,
{
    pub fn new(sink: AnySink<(Id, Output), Error>, receiver: Receiver<(Id, Output)>) -> Self {
        MultiplexerSender { sink, receiver }
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        while let Some(output) = self.receiver.next().await {
            self.sink.send(output).await?;
        }
        Ok(())
    }
}
