use std::sync::Arc;

use crate::multiplexer::MultiplexerOutgoing;
use futures::{Sink, SinkExt};
use futures_any::prelude::*;
use std::sync::Mutex;

pub trait MuxSink<Item>: Sink<Item> {
    fn mux_outgoing<MX, Output>(
        self,
        mx: Arc<Mutex<MX>>,
    ) -> AnySink<(MX::Id, Output), anyhow::Error>
    where
        MX: MultiplexerOutgoing<Output, MuxOutput = Item>,
        Self: Sized + Unpin,
        MX::Error: std::error::Error + Send + Sync + 'static,
        Self::Error: std::error::Error + Send + Sync + 'static,
    {
        self.with(move |(id, output)| {
            let mx_output = mx.lock().unwrap().outgoing(output, id);

            match mx_output {
                Ok(item) => futures::future::ok(item),
                Err(err) => futures::future::err(err.into()),
            }
        })
        .to_any_sink()
    }
}

impl<T, Item> MuxSink<Item> for T where T: Sink<Item> {}
