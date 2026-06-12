use rio_backend::event::{EventListener, RioEvent, WindowId};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Default)]
pub struct WasmListener {
    bell: Rc<Cell<bool>>,
}

impl WasmListener {
    pub fn take_bell(&self) -> bool {
        self.bell.replace(false)
    }
}

impl EventListener for WasmListener {
    fn event(&self) -> (Option<RioEvent>, bool) {
        (None, false)
    }

    fn send_event(&self, event: RioEvent, _id: WindowId) {
        if matches!(event, RioEvent::Bell) {
            self.bell.set(true);
        }
    }
}
