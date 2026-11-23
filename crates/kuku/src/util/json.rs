use std::io::{self, Write};

use serde::Serialize;

pub(crate) fn write_bounded_json<W: Write>(
    value: &impl Serialize,
    bytes: usize,
    output: W,
) -> serde_json::Result<W> {
    let mut writer = BoundedWriter { output, bytes };
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.output)
}

struct BoundedWriter<W> {
    output: W,
    bytes: usize,
}

impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized JSON exceeds byte limit",
            ));
        }
        let count = self.output.write(input)?;
        self.bytes -= count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}
