use std::sync::Arc;

use crate::multiplexer::MultiplexerOutgoing;
use futures::{Sink, SinkExt};
use futures_any::prelude::*;
use std::sync::Mutex;

pub trait MuxSink<Id, Item>: Sink<Item> {
    fn mux_outgoing<MX, Output>(self, mx: Arc<Mutex<MX>>) -> AnySink<(Id, Output), Self::Error>
    where
        MX: MultiplexerOutgoing<Output, Id, MuxOutput = Item>,
        Self: Sized + Unpin,
        Self::Error: From<MX::Error>,
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
