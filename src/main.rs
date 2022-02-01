use std::io::Write;

use ariadne::{Label, ReportKind, Source};
use crossterm::{
    cursor::{self, MoveRight, MoveTo, MoveToNextLine},
    event::{Event, KeyCode, KeyModifiers},
    style::{Color, Colors, ResetColor, SetColors},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, DisableLineWrap, EnterAlternateScreen,
        LeaveAlternateScreen, ScrollDown,
    },
    QueueableCommand,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Output {
    general_diagnostics: Vec<Diagnostic>,
    // summary: Summary,
}

#[derive(Deserialize)]
struct Diagnostic {
    file: String,
    severity: Severity,
    message: String,
    range: Range,
    rule: Option<String>,
}

#[derive(Deserialize)]
struct Range {
    start: Location,
    end: Location,
}

#[derive(Debug, Deserialize)]
struct Location {
    line: usize,
    character: usize,
}

impl Location {
    fn to_byte_offset(&self, source: &Source) -> usize {
        source.line(self.line).unwrap().offset() + self.character
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Error,
    Warning,
    Information,
}

impl Severity {
    fn to_report_kind(&self) -> ReportKind {
        match self {
            Self::Error => ReportKind::Error,
            Self::Warning => ReportKind::Warning,
            Self::Information => ReportKind::Advice,
        }
    }
}

#[derive(Deserialize)]
struct Summary {}

fn pyright() -> Vec<Item> {
    let out = std::process::Command::new("pyright")
        .arg("--outputjson")
        .arg(".")
        .output()
        .expect("No pyright in $PATH");
    if out.stdout.len() == 0 {
        return Vec::new();
    }
    let output: Output = serde_json::from_slice(&out.stdout).unwrap();
    let mut items = vec![];
    // let mut reports = vec![];
    for item in output.general_diagnostics {
        let source = Source::from(std::fs::read_to_string(&item.file).unwrap());

        let mut buf = Vec::<u8>::new();

        ariadne::Report::build(
            item.severity.to_report_kind(),
            &item.file,
            item.range.start.to_byte_offset(&source),
        )
        .with_message(format!("[pyright] {}", item.rule.unwrap_or_default()))
        .with_label(
            Label::new((
                &item.file,
                item.range.start.to_byte_offset(&source)..item.range.end.to_byte_offset(&source),
            ))
            .with_message(item.message),
        )
        .finish()
        // .print((&item.file, source))
        .write((&item.file, source), &mut buf)
        .unwrap();

        items.push(Item {
            lines: buf
                .split(|b| *b == b'\n')
                .map(|slice| slice.to_vec())
                .collect(),
        });
    }

    items
}

fn flake8() -> Vec<Item> {
    let out = std::process::Command::new("flake8")
        .arg(".")
        .output()
        .expect("No flake8 in $PATH");
    if out.stdout.len() == 0 {
        return Vec::new();
    }
    let s = std::str::from_utf8(&out.stdout).unwrap();
    let mut items = vec![];
    for line in s.lines() {
        // util/iter util.py:1:1: F821 undefined name 'f'
        let mut tokens = line.split(':');
        let path = tokens.next().unwrap();
        let row = tokens.next().unwrap().parse::<usize>().unwrap();
        let col = tokens.next().unwrap().parse::<usize>().unwrap();
        let rest = tokens.next().unwrap();
        let code = &rest[1..5];
        let msg = &rest[6..];

        let source = Source::from(std::fs::read_to_string(path).unwrap());
        let offset = Location {
            line: row - 1,
            character: col - 1,
        }
        .to_byte_offset(&source);
        let end = source.line(row - 1).unwrap().offset() + source.line(row - 1).unwrap().len();

        // https://flake8.pycqa.org/en/2.6.0/warnings.html
        let report_kind = match code.chars().next().unwrap() {
            'E' => ReportKind::Error,
            'W' => ReportKind::Warning,
            'F' => ReportKind::Error, // TODO
            'C' => ReportKind::Advice,
            'N' => ReportKind::Warning,
            _ => unreachable!(),
        };

        let mut buf = Vec::<u8>::new();
        ariadne::Report::<(&str, std::ops::Range<usize>)>::build(report_kind, path, offset)
            .with_message("[flake8]")
            .with_label(Label::new((path, offset..end)).with_message(msg))
            .finish()
            .write((path, source), &mut buf)
            .unwrap();

        items.push(Item {
            lines: buf
                .split(|b| *b == b'\n')
                .map(|slice| slice.to_vec())
                .collect(),
        });
    }
    items
}

fn render_in_buffer() -> Vec<Item> {
    let mut items = pyright();
    let items2 = flake8();
    items.extend(items2);
    items
}

struct Item {
    lines: Vec<Vec<u8>>,
}

#[derive(Default)]
struct App {
    items: Vec<Item>,
    line_offsets: Vec<usize>,
    first_visible_item: usize,
    first_visible_subline: usize,
    selected_item: usize,
    width: u16,
    height: u16,
}

impl App {
    fn render_to_term(&mut self, w: &mut impl Write) {
        let mut item = self.first_visible_item;
        let mut subline = self.first_visible_subline;

        w.queue(Clear(ClearType::All)).unwrap();
        w.queue(MoveTo(1, 1)).unwrap();

        for _row in 0..self.height {
            if item >= self.items.len() {
                break;
            }
            if subline == 0 {
                if item == self.selected_item {
                    w.write_all("▷ ".as_bytes()).unwrap();
                } else {
                    w.queue(MoveRight(3)).unwrap();
                }

                w.queue(SetColors(Colors::new(Color::Black, Color::Red)))
                    .unwrap();
                write!(w, " {} ", item + 1).unwrap();
                w.queue(ResetColor).unwrap();
                write!(w, " ").unwrap();
            } else {
                w.queue(MoveRight(3)).unwrap();
            }
            w.write_all(&self.items[item].lines[subline]).unwrap();
            w.queue(MoveToNextLine(1)).unwrap();

            subline += 1;
            if subline >= self.items[item].lines.len() {
                item += 1;
                subline = 0;
            }
        }
        w.flush().unwrap();
    }

    fn line_offset(&self, item: usize, subline: usize) -> usize {
        self.line_offsets[item] + subline
    }
}

fn main() {
    env_logger::init();

    let mut w = std::io::BufWriter::new(std::io::stdout());
    w.queue(EnterAlternateScreen).unwrap();
    w.queue(cursor::Hide).unwrap();
    w.queue(DisableLineWrap).unwrap();
    enable_raw_mode().unwrap();

    let (width, height) = crossterm::terminal::size().unwrap();
    let mut app = App {
        items: render_in_buffer(),
        width,
        height,
        ..Default::default()
    };
    app.line_offsets = {
        let mut offsets = vec![];
        let mut offset = 0;
        for item in &app.items {
            offsets.push(offset);
            offset += item.lines.len();
        }
        offsets
    };
    app.render_to_term(&mut w);

    loop {
        match crossterm::event::read().unwrap() {
            Event::Key(ev) => {
                match (ev.modifiers, ev.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c'))
                    | (KeyModifiers::CONTROL, KeyCode::Char('C')) => {
                        break;
                    }
                    _ => {}
                }
                if ev.modifiers != KeyModifiers::NONE {
                    continue;
                }
                match ev.code {
                    KeyCode::Up => {
                        if app.selected_item > 0 {
                            app.selected_item -= 1;
                            if app.first_visible_item >= app.selected_item {
                                app.first_visible_item = app.selected_item;
                                app.first_visible_subline = 0;
                            }
                            app.render_to_term(&mut w);
                        }
                    }
                    KeyCode::Down => {
                        if app.selected_item + 1 < app.items.len() {
                            app.selected_item += 1;

                            let last_visible_offset = app
                                .line_offset(app.first_visible_item, app.first_visible_subline)
                                + app.height as usize
                                - 1;

                            let selected_last_offset = app.line_offset(
                                app.selected_item,
                                app.items[app.selected_item].lines.len() - 1,
                            );

                            if last_visible_offset < selected_last_offset {
                                let delta = selected_last_offset - last_visible_offset;
                                for _ in 0..delta {
                                    app.first_visible_subline += 1;
                                    if app.first_visible_subline
                                        > app.items[app.first_visible_item].lines.len() - 1
                                    {
                                        app.first_visible_item += 1;
                                        app.first_visible_subline = 0;
                                    }
                                }
                            }

                            app.render_to_term(&mut w);
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('Q') | KeyCode::Char('q') => {
                        break;
                    }
                    KeyCode::PageDown => {
                        w.queue(ScrollDown(10)).unwrap();
                        // app.render_to_term(&mut w);
                    }
                    _ => {}
                }
            }
            Event::Resize(width, height) => {
                app.width = width;
                app.height = height;
                app.render_to_term(&mut w);
            }
            _ => {}
        }
    }

    disable_raw_mode().unwrap();
    w.queue(cursor::Show).unwrap();
    w.queue(LeaveAlternateScreen).unwrap();
    w.flush().unwrap();
}
