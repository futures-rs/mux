//! Traits for multiplexing incoming stream and outgoing sink
//!
//! The traits in this model provide mux data pack/unpack functions
//!
//! - Implement the [`MultiplexerIncoming`] trait for unpack incoming mux datagram to underlying protocol datagram
//! - Implement the [`MultiplexerOutgoing`] trait for pack underlying protocol datagram to mux datagram
//!

pub trait MultiplexerIncoming<MuxInput> {
    type Error: std::error::Error;

    type Input;

    type Id;

    fn incoming(&mut self, data: MuxInput) -> Result<(Self::Id, Self::Input), Self::Error>;
}

pub trait MultiplexerOutgoing<Output, Id> {
    type Error: std::error::Error;

    type MuxOutput;

    /// Pack underlying protocol output datagram to mux datagram and return new channel id if necessary.
    ///
    /// * `id` - The outgoing mux channel id, if id is `None` indicate that this is a new channel outgoing data
    fn outgoing(&mut self, data: Output, id: Id) -> Result<Self::MuxOutput, Self::Error>;

    /// Create new channel and return channel id
    fn connect(&mut self) -> Result<Id, Self::Error>;
}
