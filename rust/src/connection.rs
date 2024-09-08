use std::io;

use futures::future::BoxFuture;

pub struct Sender {
    inner: Box<dyn Fn(&[u8]) -> BoxFuture<'static, io::Result<()>> + Send>,
}

impl Sender {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&[u8]) -> BoxFuture<'static, io::Result<()>> + Send + 'static,
    {
        Sender { inner: Box::new(f) }
    }

    pub async fn send(&self, data: &[u8]) -> io::Result<()> {
        (self.inner)(data).await
    }
}
