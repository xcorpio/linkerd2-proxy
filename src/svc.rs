pub extern crate linkerd2_stack as stack;
extern crate tower_service;

pub use self::tower_service::{NewService, Service};

pub use self::stack::{
    Layer,
    Make,
};

#[derive(Clone, Debug)]
pub struct Shared<V>(V);

impl<V: Clone> Shared<V> {
    pub fn new(v: V) -> Self {
        Shared(v)
    }
}

impl<T, V: Clone> Make<T> for Shared<V> {
    type Value = V;
    type Error = ();

    fn make(&self, _: &T) -> Result<V, ()> {
        Ok(self.0.clone())
    }
}
