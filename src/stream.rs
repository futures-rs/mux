use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};
use futures_any::prelude::*;

use crate::multiplexer::MultiplexerIncoming;

pub trait MuxStream: Stream {
    fn mux_incoming<MX, Id, Input>(
        self,
        mx: Arc<Mutex<MX>>,
    ) -> AnyStream<Result<(Id, Option<MX::Input>), anyhow::Error>>
    where
        MX: MultiplexerIncoming<Self::Item, Input = Input, Id = Id>,
        Self: Sized + Unpin,
        MX::Error: std::error::Error + Send + Sync + 'static,
        Id: Clone,
    {
        self.flat_map(move |item| {
            log::debug!("incoming ...");

            match mx.lock().unwrap().incoming(item) {
                Ok((id, input, closed)) => {
                    let stream = if closed {
                        futures::stream::iter(vec![Ok((id.clone(), Some(input))), Ok((id, None))])
                    } else {
                        futures::stream::iter(vec![Ok((id.clone(), Some(input)))])
                    };

                    stream.to_any_stream()
                }
                Err(err) => futures::stream::iter(vec![Err::<
                    (Id, Option<MX::Input>),
                    anyhow::Error,
                >(err.into())])
                .to_any_stream(),
            }
        })
        .to_any_stream()
    }
}

impl<T> MuxStream for T where T: Stream {}
