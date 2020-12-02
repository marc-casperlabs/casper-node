use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader},
};

use serde::Deserialize;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
struct Opts {
    /// Dump malformed lines
    #[structopt(long, short)]
    dump_malformed: bool,
    /// Remove leading logging preambles from lines.
    #[structopt(long, short)]
    strip_preamble: bool,
    /// Number of fields to cut when removing preamble.
    #[structopt(long, default_value = "2")]
    cuts: usize,
}

/// A filter that strips fields from lines.
///
/// Removes the first `index` fields, assumed to be separated by any amount of whitespace.
struct FieldStripper<I> {
    lines: I,
    field: usize,
}

impl<I> FieldStripper<I> {
    /// Creates a new field stripper.
    fn new(field: usize, lines: I) -> FieldStripper<I> {
        FieldStripper { field, lines }
    }
}

impl<I: Iterator<Item = io::Result<String>>> Iterator for FieldStripper<I> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = match self.lines.next()? {
            Ok(line) => line,
            Err(err) => return Some(Err(err)),
        };

        let mut chars = line.chars().enumerate();
        for _ in 0..self.field {
            chars
                .by_ref()
                .skip_while(|(_, c)| !c.is_whitespace())
                .next();
            chars.by_ref().skip_while(|(_, c)| c.is_whitespace()).next();
        }

        if let Some((idx, _)) = chars.next() {
            Some(Ok(line.split_off(idx - 1)))
        } else {
            None
        }
    }
}

type LineIter = Box<dyn Iterator<Item = io::Result<String>>>;

#[derive(Debug, Deserialize)]
struct LogMessage {
    timestamp: String,
    level: String,
    fields: BTreeMap<String, serde_json::Value>,
    target: String,
    span: Option<serde_json::Value>,
    spans: Option<serde_json::Value>,
}

// {"timestamp":"Dec 02 01:16:28.208","level":"DEBUG","fields":{"event":"storage request: put
// executed block e2b7..8154, parent hash 2697..e321, post-state hash 3913..3cc5, body hash
// 0e57..e3a8, deploys [], random bit true, timestamp 2020-12-02T01:15:44.512Z, era_id 0, height 14,
// proofs count 1","q":"Regular"},"target":"casper_node::reactor","span":{"ev":1265,"name":"dispatch
// events"},"spans":[{"ev":1265,"name":"crank"},{"ev":1265,"name":"dispatch events"}]}

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();

    let raw_lines = BufReader::new(io::stdin()).lines();
    let line_reader: LineIter = if opts.strip_preamble {
        Box::new(raw_lines) as LineIter
    } else {
        Box::new(FieldStripper::new(opts.cuts, raw_lines)) as LineIter
    };

    for (line_num, line) in line_reader.enumerate() {
        let line = line?;

        match serde_json::from_str::<LogMessage>(&line) {
            Ok(msg) => {
                println!("{:?}", msg);
            }
            Err(err) => {
                eprintln!("Could not parse log line {}: {}", line_num, err);
                if opts.dump_malformed {
                    eprintln!("{}", line);
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::FieldStripper;
    use std::io;

    #[test]
    fn test_split_non_whitespace() {
        let inputs = vec!["foo bar baz", "a b c d e f"];

        let fs = FieldStripper::new(Some(' '), 2, inputs.into_iter().map(str::to_owned).map(Ok));
        let result: io::Result<Vec<_>> = fs.collect();
        let output = result.unwrap();

        assert_eq!(output, vec!["baz".to_string(), "c d e f".to_string()])
    }

    #[test]
    fn test_split_whitespace() {
        let inputs = vec!["foo    bar  x baz"];

        let fs = FieldStripper::new(None, 2, inputs.into_iter().map(str::to_owned).map(Ok));
        let result: io::Result<Vec<_>> = fs.collect();
        let output = result.unwrap();

        assert_eq!(output, vec!["x baz".to_string()])
    }
}
