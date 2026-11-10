//! The holder's reply framing. Body lines beginning with `%` acquire an
//! extra `%` on the wire, so no captured text can impersonate a marker.

use anyhow::{Result, bail};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Append a reply body, escaping each line that could look like a marker.
pub fn escape_body(out: &mut String, body: &str) {
    for line in body.split_inclusive('\n') {
        if line.starts_with('%') {
            out.push('%');
        }
        out.push_str(line);
    }
}

#[derive(Default)]
pub struct Framing {
    number: Option<String>,
}

pub enum Line<'a> {
    Skip,
    Body(&'a str),
    End(bool),
}

impl Framing {
    /// Classify a line without its newline, unescaping body text only.
    pub fn line<'a>(&mut self, line: &'a str) -> Line<'a> {
        let Some(number) = &self.number else {
            if let Some(rest) = line.strip_prefix("%begin ")
                && let Some(number) = field(rest)
            {
                self.number = Some(number.to_owned());
            }
            return Line::Skip;
        };
        let end = [("%end ", true), ("%error ", false)]
            .into_iter()
            .find_map(|(word, ok)| Some((ok, field(line.strip_prefix(word)?)?)));
        if let Some((ok, closing)) = end
            && closing == number
        {
            self.number = None;
            return Line::End(ok);
        }
        Line::Body(if line.starts_with("%%") {
            &line[1..]
        } else {
            line
        })
    }
}

/// The command number in `<time> <number> <flags>`.
fn field(rest: &str) -> Option<&str> {
    rest.split(' ').nth(1)
}

#[derive(Default)]
pub struct Reply {
    framing: Framing,
    lines: Vec<String>,
}

impl Reply {
    /// Takes one line; returns the reply it completes.
    pub fn line(&mut self, line: &str) -> Option<Result<String, String>> {
        match self.framing.line(line) {
            Line::Skip => None,
            Line::Body(body) => {
                self.lines.push(body.to_owned());
                None
            }
            Line::End(ok) => {
                let body = std::mem::take(&mut self.lines).join("\n");
                Some(if ok { Ok(body) } else { Err(body) })
            }
        }
    }
}

/// Stream one reply without collecting its body.
pub async fn stream<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    out: &mut W,
) -> Result<()> {
    let mut framing = Framing::default();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            bail!("the holder closed the connection");
        }
        let text = line.strip_suffix('\n').unwrap_or(&line);
        match framing.line(text) {
            Line::Skip => {}
            Line::Body(body) => {
                out.write_all(body.as_bytes()).await?;
                out.write_all(b"\n").await?;
            }
            Line::End(true) => {
                out.flush().await?;
                return Ok(());
            }
            Line::End(false) => bail!("holder command failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "%end 0 1 1\n%error 0 1 1\n%%two\n%\nlast";

    fn wire(body: &str, ending: &str) -> String {
        let mut text = String::from("%sessions-changed\n%begin 9 1 1\n");
        escape_body(&mut text, body);
        text.push('\n');
        text.push_str(ending);
        text
    }

    #[test]
    fn collected_reply_preserves_marker_shaped_body() {
        let wire = wire(BODY, "%end 9 1 1\n");
        let mut reply = Reply::default();
        assert_eq!(
            wire.lines().find_map(|line| reply.line(line)),
            Some(Ok(BODY.into()))
        );
    }

    #[test]
    fn errors_carry_their_message() {
        let wire = wire(BODY, "%error 9 1 1\n");
        let mut reply = Reply::default();
        assert_eq!(
            wire.lines().find_map(|line| reply.line(line)),
            Some(Err(BODY.into()))
        );
    }

    #[tokio::test]
    async fn streamed_reply_preserves_marker_shaped_body() {
        let wire = wire(BODY, "%end 9 1 1\n");
        let mut input = tokio::io::BufReader::new(wire.as_bytes());
        let mut output = Vec::new();
        stream(&mut input, &mut output).await.unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), format!("{BODY}\n"));
    }
}
