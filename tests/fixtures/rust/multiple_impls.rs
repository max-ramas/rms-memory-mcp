pub struct User;

/// Construction methods.
impl User {
    pub fn new() -> Self {
        Self
    }
}

/// Query methods.
impl User {
    pub fn name(&self) -> &'static str {
        "user"
    }
}
