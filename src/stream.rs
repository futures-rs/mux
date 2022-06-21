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
        self.flat_map(move |item| {
            log::debug!("incoming ...");

            match mx.lock().unwrap().incoming(item) {
                Ok(data) => futures::stream::iter(
                    data.into_iter()
                        .map(|(id, item)| Ok((id, item)))
                        .collect::<Vec<_>>(),
                )
                .to_any_stream(),
                Err(err) => {
                    log::debug!("incoming ... err");
                    let stream = futures::stream::once(async {
                        Err::<(Id, MX::Input), anyhow::Error>(err.into())
                    });

                    futures::pin_mut!(stream);

                    stream.to_any_stream()
                }
            }
        })
        .to_any_stream()
    }
}

impl<T> MuxStream for T where T: Stream {}
