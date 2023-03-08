//! Adaptation/port of [Go scanner](http://tip.golang.org/pkg/bufio/#Scanner).

use log::debug;

use std::error::Error;
use std::fmt;
use std::io;

#[cfg(feature = "buf_redux")]
use buf_redux::Buffer;

use super::sql::Token;
#[cfg(feature = "buf_redux")]
const MAX_CAPACITY: usize = 1024 * 1024 * 1024;

pub trait Input: fmt::Debug {
    fn fill_buf(&mut self) -> io::Result<()>; // -> io::Result<&[u8]>;
    fn eof(&self) -> bool; //&mut self -> io::Result<bool>
    fn consume(&mut self, amount: usize); // -> &[u8]
    fn buffer(&self) -> &[u8];
    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
}

/// Memory input
impl Input for &[u8] {
    #[inline]
    fn fill_buf(&mut self) -> io::Result<()> {
        Ok(())
    }

    #[inline]
    fn eof(&self) -> bool {
        true
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        *self = &self[amt..];
    }

    #[inline]
    fn buffer(&self) -> &[u8] {
        self
    }

    #[inline]
    fn is_empty(&self) -> bool {
        (*self).is_empty()
    }

    #[inline]
    fn len(&self) -> usize {
        (*self).len()
    }
}

impl Input for Vec<u8> {
    #[inline]
    fn fill_buf(&mut self) -> io::Result<()> {
        Ok(())
    }

    #[inline]
    fn eof(&self) -> bool {
        true
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        self.drain(..amt);
    }

    #[inline]
    fn buffer(&self) -> &[u8] {
        self
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }
}

/// Streaming input
#[cfg(feature = "buf_redux")]
pub struct InputStream<R> {
    /// The reader provided by the client.
    inner: R,
    /// Buffer used as argument to split.
    buf: Buffer,
    eof: bool,
}

#[cfg(feature = "buf_redux")]
impl<R: io::Read> InputStream<R> {
    pub fn new(inner: R) -> Self {
        Self::with_capacity(inner, 4096)
    }

    fn with_capacity(inner: R, capacity: usize) -> Self {
        let buf = Buffer::with_capacity_ringbuf(capacity);
        InputStream {
            inner,
            buf,
            eof: false,
        }
    }
}

#[cfg(feature = "buf_redux")]
impl<R: io::Read> Input for InputStream<R> {
    fn fill_buf(&mut self) -> io::Result<()> {
        debug!(target: "scanner", "fill_buf: {}", self.buf.capacity());
        // Is the buffer full? If so, resize.
        if self.buf.free_space() == 0 {
            let mut capacity = self.buf.capacity();
            if capacity * 2 < MAX_CAPACITY {
                capacity *= 2;
                self.buf.make_room();
                self.buf.reserve(capacity);
            } else {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof)); // FIXME
            }
        } else if self.buf.usable_space() == 0 {
            self.buf.make_room();
        }
        // Finally we can read some input.
        let sz = self.buf.read_from(&mut self.inner)?;
        self.eof = sz == 0;
        Ok(())
    }

    #[inline]
    fn eof(&self) -> bool {
        self.eof
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        self.buf.consume(amt);
    }

    #[inline]
    fn buffer(&self) -> &[u8] {
        self.buf.buf()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[inline]
    fn len(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(feature = "buf_redux")]
impl<R> fmt::Debug for InputStream<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputStream")
            .field("input", &self.buf)
            .field("eof", &self.eof)
            .finish()
    }
}

pub trait ScanError: Error + From<io::Error> + Sized {
    fn position(&mut self, line: u64, column: usize);
}

/// The `(&[u8], TokenType)` is the token.
/// And the `usize` is the amount of bytes to consume.
pub(crate) type SplitResult<TokenType, Error> = Result<(Option<Token<TokenType>>, usize), Error>;

/// Split function used to tokenize the input
pub trait Splitter: Sized {
    type Error: ScanError;
    //type Item: ?Sized;
    type TokenType;

    /// The arguments are an initial substring of the remaining unprocessed
    /// data and a flag, `eof`, that reports whether the Reader has no more data
    /// to give.
    ///
    /// If the returned error is non-nil, scanning stops and the error
    /// is returned to the client.
    ///
    /// The function is never called with an empty data slice unless at EOF.
    /// If `eof` is true, however, data may be non-empty and,
    /// as always, holds unprocessed text.
    fn split(&mut self, data: &[u8], eof: bool) -> SplitResult<Self::TokenType, Self::Error>;
}

/// Like a `BufReader` but with a growable buffer.
/// Successive calls to the `scan` method will step through the 'tokens'
/// of a file, skipping the bytes between the tokens.
///
/// Scanning stops unrecoverably at EOF, the first I/O error, or a token too
/// large to fit in the buffer. When a scan stops, the reader may have
/// advanced arbitrarily far past the last token.
pub struct Scanner<I: Input, S: Splitter> {
    /// The reader provided by the client.
    input: I,
    /// The function to tokenize the input.
    splitter: S,
    /// current line number
    line: u64,
    /// current column number (byte offset, not char offset)
    column: usize,
}

impl<I: Input, S: Splitter> Scanner<I, S> {
    pub fn new(input: I, splitter: S) -> Scanner<I, S> {
        Scanner {
            input,
            splitter,
            line: 1,
            column: 1,
        }
    }

    /// Current line number
    pub fn line(&self) -> u64 {
        self.line
    }

    /// Current column number (byte offset, not char offset)
    pub fn column(&self) -> usize {
        self.column
    }

    pub fn splitter(&self) -> &S {
        &self.splitter
    }

    /// Reset the scanner such that it behaves as if it had never been used.
    pub fn reset(&mut self, input: I) {
        self.input = input;
        self.line = 1;
        self.column = 1;
    }
}

type ScanResult<TokenType, Error> = Result<Option<Token<TokenType>>, Error>;

impl<I: Input, S: Splitter> Scanner<I, S> {
    /// Advance the Scanner to next token.
    /// Return the token as a byte slice.
    /// Return `None` when the end of the input is reached.
    /// Return any error that occurs while reading the input.
    pub fn scan(&mut self) -> ScanResult<S::TokenType, S::Error> {
        debug!(target: "scanner", "scan(line: {}, column: {})", self.line, self.column);
        // Loop until we have a token.
        loop {
            let eof = self.input.eof();
            // See if we can get a token with what we already have.
            if !self.input.is_empty() || eof {
                match self.splitter.split(self.input.buffer(), eof) {
                    Err(mut e) => {
                        e.position(self.line, self.column);
                        return Err(e);
                    }
                    Ok((None, 0)) => {
                        // Request more data
                    }
                    Ok((None, amt)) => {
                        // Ignore/skip this data
                        self.consume(amt);
                        continue;
                    }
                    Ok((tok, amt)) => {
                        self.consume(amt);
                        return Ok(tok);
                    }
                }
            }
            // We cannot generate a token with what we are holding.
            // If we've already hit EOF, we are done.
            if eof {
                // Shut it down.
                return Ok(None);
            }
            // Must read more data.
            self.input.fill_buf()?;
        }
    }

    /// Consume `amt` bytes of the buffer.
    fn consume(&mut self, amt: usize) {
        debug!(target: "scanner", "consume({})", amt);
        debug_assert!(amt <= self.input.len());
        for byte in &self.input.buffer()[..amt] {
            if *byte == b'\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        self.input.consume(amt);
    }
}

impl<I: Input, S: Splitter> fmt::Debug for Scanner<I, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scanner")
            .field("input", &self.input)
            .field("line", &self.line)
            .field("column", &self.column)
            .finish()
    }
}
