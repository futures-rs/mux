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

    /// The success returns value is tuple (id,input,disconnect_flag)
    fn incoming(&mut self, data: MuxInput) -> Result<(Self::Id, Self::Input, bool), Self::Error>;

    fn disconnect(&mut self, id: Self::Id);
}

pub trait MultiplexerOutgoing<Output> {
    type Error: std::error::Error;

    type MuxOutput;

    type Id;

    /// Pack underlying protocol output datagram to mux datagram and return new channel id if necessary.
    ///
    /// * `id` - The outgoing mux channel id, if id is `None` indicate that this is a new channel outgoing data
    fn outgoing(&mut self, data: Output, id: Self::Id) -> Result<Self::MuxOutput, Self::Error>;

    /// Create new channel and return channel id
    fn connect(&mut self) -> Result<Self::Id, Self::Error>;
}
