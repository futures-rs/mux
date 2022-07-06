//! Traits for multiplexing incoming stream and outgoing sink
//!
//! The traits in this model provide mux data pack/unpack functions
//!
//! - Implement the [`MultiplexerIncoming`] trait for unpack incoming mux datagram to underlying protocol datagram
//! - Implement the [`MultiplexerOutgoing`] trait for pack underlying protocol datagram to mux datagram

use std::{
    fmt::{Debug, Display},
    io,
};

pub trait MultiplexerIncoming<MuxInput> {
    type Error: From<io::Error>;

    type Input: Debug;

    type Id: Display;

    /// The success returns value is tuple (id,input,disconnect_flag)
    fn incoming(&mut self, data: MuxInput) -> Result<(Self::Id, Self::Input, bool), Self::Error>;

    /// Disconnect channel by id
    fn disconnect(&mut self, id: Self::Id);
}

pub trait MultiplexerOutgoing<Output> {
    type Error: From<io::Error>;

    type MuxOutput: Debug;

    type Id: Display;

    /// Pack underlying protocol output datagram to mux datagram and return new channel id if necessary.
    fn outgoing(&mut self, data: Output, id: Self::Id) -> Result<Self::MuxOutput, Self::Error>;

    /// Create new channel and return channel id
    fn connect(&mut self) -> Result<Self::Id, Self::Error>;
}
