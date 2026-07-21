/// Stateful normalizer for bytes written to a Windows pseudoconsole.
#[derive(Default)]
pub struct WindowsTtyInputNormalizer {
    previous_was_cr: bool,
}

impl WindowsTtyInputNormalizer {
    pub fn normalize(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut normalized = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            match byte {
                b'\x08' => normalized.push(b'\x7f'),
                b'\n' => {
                    if !self.previous_was_cr {
                        normalized.push(b'\r');
                    }
                }
                _ => normalized.push(byte),
            }
            self.previous_was_cr = byte == b'\r';
        }
        normalized
    }
}
