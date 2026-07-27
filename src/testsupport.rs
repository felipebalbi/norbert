//! Shared test doubles.
#![cfg(test)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::ui::{Mode, Ui};

/// A `Write` sink that captures bytes for assertions.
#[derive(Clone, Default)]
pub struct Buf(Arc<Mutex<Vec<u8>>>);

impl Buf {
    pub fn contents(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Buf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Ui {
    /// A `Ui` in `mode` writing to captured buffers; returns `(ui, out, err)`.
    pub fn captured(mode: Mode) -> (Ui, Buf, Buf) {
        let out = Buf::default();
        let err = Buf::default();
        let ui = Ui::from_parts(mode, Box::new(out.clone()), Box::new(err.clone()));
        (ui, out, err)
    }
}
