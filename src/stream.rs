use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};
use futures_any::prelude::*;

use crate::multiplexer::MultiplexerIncoming;

pub trait MuxStream: Stream {
    fn mux_incoming<MX, Id, Input>(
        self,
        mx: Arc<Mutex<MX>>,
    ) -> AnyStream<Result<(Id, MX::Input), anyhow::Error>>
    where
        MX: MultiplexerIncoming<Self::Item, Input = Input, Id = Id>,
        Self: Sized + Unpin,
        MX::Error: std::error::Error + Send + Sync + 'static,
    {
        self.map(move |item| mx.lock().unwrap().incoming(item).map_err(|err| err.into()))
            .to_any_stream()
    }
}

impl<T> MuxStream for T where T: Stream {}
