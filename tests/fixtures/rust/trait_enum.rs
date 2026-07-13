/// Loading behavior.
pub trait Load {
    fn load(&self);
}

/// Runtime state.
pub enum State {
    Ready,
    Failed,
}
