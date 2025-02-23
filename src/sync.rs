use super::*;
use std::convert::TryFrom;
use std::io::Cursor;
use std::prelude::v1::*;

/// Generic synchronous implementation of DFU.
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub struct DfuSync<IO, E>
where
    IO: DfuIo<Read = usize, Write = usize, Reset = (), Error = E>,
    E: From<std::io::Error> + From<Error>,
{
    dfu: DfuSansIo<IO>,
    buffer: Vec<u8>,
    progress: Option<Box<dyn FnMut(usize)>>,
}

impl<IO, E> DfuSync<IO, E>
where
    IO: DfuIo<Read = usize, Write = usize, Reset = (), Error = E>,
    E: From<std::io::Error> + From<Error>,
{
    /// Create a new instance of a generic synchronous implementation of DFU.
    pub fn new(io: IO) -> Self {
        let transfer_size = io.functional_descriptor().transfer_size as usize;

        Self {
            dfu: DfuSansIo::new(io),
            buffer: vec![0x00; transfer_size],
            progress: None,
        }
    }

    /// Use this closure to show progress.
    pub fn with_progress(&mut self, progress: impl FnMut(usize) + 'static) -> &mut Self {
        self.progress = Some(Box::new(progress));
        self
    }

    /// Consume the object and return its [`DfuIo`]
    pub fn into_inner(self) -> IO {
        self.dfu.into_inner()
    }
}

impl<IO, E> DfuSync<IO, E>
where
    IO: DfuIo<Read = usize, Write = usize, Reset = (), Error = E>,
    E: From<std::io::Error> + From<Error>,
{
    /// Download a slice to on to the device.
    pub fn download_from_slice(&mut self, slice: &[u8]) -> Result<(), IO::Error> {
        let length = slice.len();
        let cursor = Cursor::new(slice);

        self.download(
            cursor,
            u32::try_from(length).map_err(|_| Error::OutOfCapabilities)?,
        )
    }

    /// Download a firmware into the device.
    pub fn download<R: std::io::Read>(&mut self, reader: R, length: u32) -> Result<(), IO::Error> {
        use std::io::BufRead;

        let transfer_size = self.dfu.io.functional_descriptor().transfer_size as usize;
        let mut reader = std::io::BufReader::with_capacity(transfer_size, reader);
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }

        macro_rules! wait_status {
            ($cmd:expr) => {{
                let mut cmd = $cmd;
                loop {
                    cmd = match cmd.next() {
                        get_status::Step::Break(cmd) => break cmd,
                        get_status::Step::Wait(cmd, poll_timeout) => {
                            std::thread::sleep(std::time::Duration::from_millis(poll_timeout));
                            let (cmd, n) = cmd.get_status(&mut self.buffer)?;
                            cmd.chain(&self.buffer[..n])??
                        }
                    };
                }
            }};
        }

        let cmd = self.dfu.download(length)?;
        let (cmd, n) = cmd.get_status(&mut self.buffer)?;
        let (cmd, _) = cmd.chain(&self.buffer[..n])??;
        let (cmd, n) = cmd.get_status(&mut self.buffer)?;
        let mut download_loop = cmd.chain(&self.buffer[..n])??;

        loop {
            download_loop = match download_loop.next() {
                download::Step::Break => break,
                download::Step::Erase(cmd) => {
                    let (cmd, _) = cmd.erase()?;
                    wait_status!(cmd)
                }
                download::Step::SetAddress(cmd) => {
                    let (cmd, _) = cmd.set_address()?;
                    wait_status!(cmd)
                }
                download::Step::DownloadChunk(cmd) => {
                    let chunk = reader.fill_buf()?;
                    let (cmd, n) = cmd.download(chunk)?;
                    reader.consume(n);
                    if let Some(progress) = self.progress.as_mut() {
                        progress(n);
                    }
                    wait_status!(cmd)
                }
                download::Step::UsbReset => {
                    log::trace!("Device reset");
                    self.dfu.io.usb_reset()?;
                    break;
                }
            }
        }

        Ok(())
    }

    /// Download a firmware into the device.
    ///
    /// The length is guest from the reader.
    pub fn download_all<R: std::io::Read + std::io::Seek>(
        &mut self,
        mut reader: R,
    ) -> Result<(), IO::Error> {
        let length = u32::try_from(reader.seek(std::io::SeekFrom::End(0))?)
            .map_err(|_| Error::MaximumTransferSizeExceeded)?;
        reader.seek(std::io::SeekFrom::Start(0))?;
        self.download(reader, length)
    }

    /// Send a Detach request to the device
    pub fn detach(&self) -> Result<(), IO::Error> {
        self.dfu.detach()
    }

    /// Reset the USB device
    pub fn usb_reset(&self) -> Result<IO::Reset, IO::Error> {
        self.dfu.usb_reset()
    }

    /// Returns whether the device is will detach if requested
    pub fn will_detach(&self) -> bool {
        self.dfu.will_detach()
    }

    /// Returns whether the device is manifestation tolerant
    pub fn manifestation_tolerant(&self) -> bool {
        self.dfu.manifestation_tolerant()
    }
}
