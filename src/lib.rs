use std::{
    fmt::{Debug, Display},
    hash::Hash,
};

use futures::{Sink, Stream, StreamExt};
use futures_any::stream_sink::{AnySinkEx, AnyStreamEx};
use multiplexing::{MultiplexerIncoming, MultiplexerOutgoing};

pub mod channel;
pub mod executor;
pub mod multiplexing;

/// Create new mux executor with input io stream/sink
pub fn new<Mux, Id, Output, Input, Error, S>(
    s: S,
    mux: Mux,
    buffer: usize,
) -> executor::Executor<Mux, Id, Output, Input, Error>
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
    S: Stream<Item = Result<Input, Error>> + Sink<Mux::MuxOutput, Error = Error>,
{
    let (w, r) = s.split();

    executor::Executor::new(mux, r.to_any_stream(), w.to_any_sink(), buffer)
}

#[cfg(feature = "use-framed")]
/// Create new mux executor with input io stream/sink
pub fn framed<Mux, Id, Output, Input, Error, IO, Codec>(
    io: IO,
    codec: Codec,
    mux: Mux,
    buffer: usize,
) -> executor::Executor<Mux, Id, Output, Input, Error>
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
    IO: futures::AsyncRead + futures::AsyncWrite,
    Codec: futures_framed::Decoder<Item = Input, Error = Error>
        + futures_framed::Encoder<Mux::MuxOutput, Error = Error>,
{
    let framed = codec.framed(io);

    new(framed, mux, buffer)
}

#[cfg(feature = "use-async-std")]
pub mod async_std {

    use super::*;
    use futures::channel::mpsc::*;

    pub fn new<Mux, Id, Output, Input, Error, S>(
        s: S,
        mux: Mux,
        buffer: usize,
    ) -> (
        Receiver<channel::Channel<Id, Output, Mux::Input, Error>>,
        impl FnMut() -> Result<channel::Channel<Id, Output, Mux::Input, Error>, Error> + 'static,
    )
    where
        Id: Display + Clone + Eq + Hash + Send + Sync,
        Output: Debug + Send + 'static,
        Input: Debug + Send + 'static,
        Error: From<std::io::Error> + Display + Send + Debug,
        Mux: MultiplexerIncoming<Input, Error = Error, Id = Id>
            + MultiplexerOutgoing<Output, Error = Error, Id = Id>
            + Send
            + 'static,
        Mux::Input: Send + 'static,
        Mux::MuxOutput: Send + 'static,
        S: Stream<Item = Result<Input, Error>> + Sink<Mux::MuxOutput, Error = Error>,
    {
        let mut executor = crate::new(s, mux, buffer);

        let connector = executor.connector();
        let incoming = executor.incoming();

        ::async_std::task::spawn(async move {
            let result = executor.join().await;

            log::info!("executor join result({:?})", result);
        });

        (incoming, connector)
    }

    #[cfg(feature = "use-framed")]
    /// Create new mux executor with input io stream/sink
    pub fn framed<Mux, Id, Output, Input, Error, IO, Codec>(
        io: IO,
        codec: Codec,
        mux: Mux,
        buffer: usize,
    ) -> (
        Receiver<channel::Channel<Id, Output, Mux::Input, Error>>,
        impl FnMut() -> Result<channel::Channel<Id, Output, Mux::Input, Error>, Error> + 'static,
    )
    where
        Id: Display + Clone + Eq + Hash + Send + Sync,
        Output: Debug + Send + 'static,
        Input: Debug + Send + 'static,
        Error: From<std::io::Error> + Display + Send + Debug,
        Mux: MultiplexerIncoming<Input, Error = Error, Id = Id>
            + MultiplexerOutgoing<Output, Error = Error, Id = Id>
            + Send
            + 'static,
        Mux::Input: Send + 'static,
        Mux::MuxOutput: Send + 'static,
        IO: futures::AsyncRead + futures::AsyncWrite + 'static,
        Codec: futures_framed::Decoder<Item = Input, Error = Error>
            + futures_framed::Encoder<Mux::MuxOutput, Error = Error>
            + 'static,
    {
        let framed = codec.framed(io);

        new(framed, mux, buffer)
    }
}

#[cfg(feature = "use-tokio")]
pub mod tokio {

    use super::*;

    use futures::channel::mpsc::*;

    pub fn new<Mux, Id, Output, Input, Error, S>(
        s: S,
        mux: Mux,
        buffer: usize,
    ) -> (
        Receiver<channel::Channel<Id, Output, Mux::Input, Error>>,
        impl FnMut() -> Result<channel::Channel<Id, Output, Mux::Input, Error>, Error> + 'static,
    )
    where
        Id: Display + Clone + Eq + Hash + Send + Sync,
        Output: Debug + Send + 'static,
        Input: Debug + Send + 'static,
        Error: From<std::io::Error> + Display + Send + Debug,
        Mux: MultiplexerIncoming<Input, Error = Error, Id = Id>
            + MultiplexerOutgoing<Output, Error = Error, Id = Id>
            + Send
            + 'static,
        Mux::Input: Send + 'static,
        Mux::MuxOutput: Send + 'static,
        S: Stream<Item = Result<Input, Error>> + Sink<Mux::MuxOutput, Error = Error>,
    {
        let mut executor = crate::new(s, mux, buffer);

        let connector = executor.connector();
        let incoming = executor.incoming();

        ::tokio::task::spawn(async move {
            let result = executor.join().await;

            log::info!("executor join result({:?})", result);
        });

        (incoming, connector)
    }

    #[cfg(feature = "use-framed")]
    /// Create new mux executor with input io stream/sink
    pub fn framed<Mux, Id, Output, Input, Error, IO, Codec>(
        io: IO,
        codec: Codec,
        mux: Mux,
        buffer: usize,
    ) -> (
        Receiver<channel::Channel<Id, Output, Mux::Input, Error>>,
        impl FnMut() -> Result<channel::Channel<Id, Output, Mux::Input, Error>, Error> + 'static,
    )
    where
        Id: Display + Clone + Eq + Hash + Send + Sync,
        Output: Debug + Send + 'static,
        Input: Debug + Send + 'static,
        Error: From<std::io::Error> + Display + Send + Debug,
        Mux: MultiplexerIncoming<Input, Error = Error, Id = Id>
            + MultiplexerOutgoing<Output, Error = Error, Id = Id>
            + Send
            + 'static,
        Mux::Input: Send + 'static,
        Mux::MuxOutput: Send + 'static,
        IO: futures::AsyncRead + futures::AsyncWrite + 'static,
        Codec: futures_framed::Decoder<Item = Input, Error = Error>
            + futures_framed::Encoder<Mux::MuxOutput, Error = Error>
            + 'static,
    {
        let framed = codec.framed(io);

        new(framed, mux, buffer)
    }
}
