use crate::client::Client;

pub struct NullClient;

impl Client for NullClient {
    fn supported(&mut self) -> bool {
        false
    }

    fn current_application(&mut self) -> Option<String> {
        None
    }

    fn current_window(&mut self) -> Option<String> {
        None
    }
}
