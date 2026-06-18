/// Application-level error log. Single-threaded (GTK main thread only).
use std::cell::RefCell;

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn push(msg: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(msg.into()));
}

pub fn entries() -> Vec<String> {
    LOG.with(|l| l.borrow().clone())
}

pub fn clear() {
    LOG.with(|l| l.borrow_mut().clear());
}
