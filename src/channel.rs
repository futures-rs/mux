use std::{
    fmt::{Debug, Display},
    task::Poll,
};

use futures::{
    channel::mpsc::{Receiver, Sender},
    Sink, SinkExt, Stream, StreamExt,
};

/// Multiplexer channel object
pub struct Channel<Id, Output, Input, Error>
where
    Id: Display + Clone + 'static,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
    id: Id,
    stream: Option<Receiver<Result<Input, Error>>>,
    sink: Option<Sender<(Id, Output)>>,
    disconnect: Option<Box<dyn FnOnce() + Send>>,
}

/// Implement display trait
impl<Id, Output, Input, Error> Display for Channel<Id, Output, Input, Error>
where
    Id: Display + Clone + 'static,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mux channel {}", self.id)
    }
}

/// Support unpin
impl<Id, Output, Input, Error> Unpin for Channel<Id, Output, Input, Error>
where
    Id: Display + Clone + 'static,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
}

/// Auto disconnect channel
impl<Id, Output, Input, Error> Drop for Channel<Id, Output, Input, Error>
where
    Id: Display + Clone + 'static,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
    fn drop(&mut self) {
        self.disconnect()
    }
}

/// Channel pub funs
impl<Id, Output, Input, Error> Channel<Id, Output, Input, Error>
where
    Id: Display + Clone + 'static,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
    pub fn new(
        id: Id,
        stream: Receiver<Result<Input, Error>>,
        sink: Sender<(Id, Output)>,
        disconnect: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            id,
            stream: Some(stream),
            sink: Some(sink),
            disconnect: Some(Box::new(disconnect)),
        }
    }

    /// The function support reentery, but only first time has side effects
    pub fn disconnect(&mut self) {
        log::debug!("disconnect {}", self.id);

        // Only execute once
        if self.disconnect.is_none() {
            return;
        }

        let disconnect = self.disconnect.take().expect("call channel close twice");

        disconnect();

        log::debug!("disconnect {} -- success", self.id.to_string());
    }

    pub fn is_disconnected(&mut self) -> bool {
        self.disconnect.is_none()
    }
}

/// Impl futures Stream trait
impl<Id, Output, Input, Error> Stream for Channel<Id, Output, Input, Error>
where
    Id: Display + Clone,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
    type Item = Result<Input, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        assert!(!self.is_disconnected(), "channel closed");

        let mut stream = self
            .stream
            .take()
            .expect("error: call channel feature twice");

        match stream.poll_next_unpin(cx) {
            Poll::Ready(Some(data)) => {
                self.stream = Some(stream);
                return Poll::Ready(Some(data));
            }
            Poll::Ready(None) => {
                self.stream = Some(stream);
                return Poll::Ready(None);
            }
            Poll::Pending => {
                self.stream = Some(stream);
                return Poll::Pending;
            }
        }
    }
}

/// Impl futures Sink
impl<Id, Output, Input, Error> Sink<Output> for Channel<Id, Output, Input, Error>
where
    Id: Display + Clone,
    Output: Debug,
    Input: Debug,
    Error: From<std::io::Error> + 'static,
{
    type Error = Error;

    fn start_send(mut self: std::pin::Pin<&mut Self>, item: Output) -> Result<(), Self::Error> {
        assert!(!self.is_disconnected(), "channel closed");

        let mut sink = self.sink.take().expect("error: call channel feature twice");

        let result = sink.start_send((self.id.clone(), item));

        self.sink = Some(sink);

        result.map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err).into())
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        assert!(!self.is_disconnected(), "channel closed");

        let mut sink = self.sink.take().expect("error: call channel feature twice");

        let result = sink.poll_close_unpin(cx);

        self.sink = Some(sink);

        result.map(|data| {
            data.map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err).into())
        })
    }

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        assert!(!self.is_disconnected(), "channel closed");

        let mut sink = self.sink.take().expect("error: call channel feature twice");

        let result = sink.poll_ready(cx);

        self.sink = Some(sink);

        result.map(|data| {
            data.map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err).into())
        })
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        assert!(!self.is_disconnected(), "channel closed");

        let mut sink = self.sink.take().expect("error: call channel feature twice");

        let result = sink.poll_flush_unpin(cx);

        self.sink = Some(sink);

        result.map(|data| {
            data.map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err).into())
        })
    }
}

#[cfg(test)]
mod tests {

    use futures::{channel::mpsc::channel, TryStreamExt};

    use super::*;

    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum TestError {
        #[error("read/write failed: {0}")]
        IOFailed(String),
    }

    impl From<std::io::Error> for TestError {
        fn from(err: std::io::Error) -> Self {
            TestError::IOFailed(format!("{}", err))
        }
    }

    #[async_std::test]
    async fn test_channel() -> anyhow::Result<()> {
        _ = pretty_env_logger::try_init();

        let (mut incoming_sender, incoming_receiver) = channel::<Result<u32, TestError>>(100);

        let (outgoing_sender, mut outgoing_receiver) = channel::<(u32, u32)>(100);

        let mut channel = Channel::new(1, incoming_receiver, outgoing_sender, || {});

        async_std::task::spawn(async move {
            while let Some(data) = channel.try_next().await? {
                channel.send(data).await?;
            }

            Ok::<(), anyhow::Error>(())
        });

        for i in 0..100 {
            incoming_sender.send(Ok(i)).await?;

            assert_eq!(outgoing_receiver.next().await, Some((1, i)));
        }

        Ok(())
    }

    #[async_std::test]
    async fn test_drop_outgoing_receiver() -> Result<(), anyhow::Error> {
        _ = pretty_env_logger::try_init();

        let (_incoming_sender, incoming_receiver) = channel::<Result<u32, TestError>>(100);

        let (outgoing_sender, outgoing_receiver) = channel::<(u32, u32)>(100);

        let mut channel = Channel::new(1, incoming_receiver, outgoing_sender, || {});

        drop(outgoing_receiver);

        match channel.send(1).await {
            Err(TestError::IOFailed(err)) => {
                assert_eq!(err, "send failed because receiver is gone");
            }
            _ => {
                assert!(false, "expect io error");
            }
        }

        Ok(())
    }

    #[async_std::test]
    async fn test_drop_incoming_sender() -> Result<(), anyhow::Error> {
        _ = pretty_env_logger::try_init();

        let (incoming_sender, incoming_receiver) = channel::<Result<u32, TestError>>(100);

        let (outgoing_sender, _outgoing_receiver) = channel::<(u32, u32)>(100);

        let mut channel = Channel::new(1, incoming_receiver, outgoing_sender, || {});

        drop(incoming_sender);

        assert_eq!(channel.try_next().await?, None, "expect Poll:Ready(None)");
        Ok(())
    }

    #[async_std::test]
    async fn test_split() -> Result<(), anyhow::Error> {
        _ = pretty_env_logger::try_init();

        let (mut incoming_sender, incoming_receiver) = channel::<Result<u32, TestError>>(100);

        let (outgoing_sender, mut outgoing_receiver) = channel::<(u32, u32)>(100);

        let channel = Channel::new(1, incoming_receiver, outgoing_sender, || {});

        let (mut w, mut r) = channel.split();

        for i in 0..100 {
            incoming_sender.send(Ok(i)).await?;

            let data = r.try_next().await?;

            assert_eq!(data, Some(i));

            w.send(data.unwrap()).await?;

            assert_eq!(outgoing_receiver.next().await, Some((1, i)));
        }

        Ok(())
    }
}
