//! A terminal REPL over nt-core, for exercising the shared data layer and for Bombadil
//! (tests/bombadil/ntrepl.spec.ts).
//!
//! Commands: home [topic], search <q>, more, open <n>, watch <id>, comments, replies <n>,
//! channel <id>, back, state, quit. Anything else prints the command list. It replays the
//! recorded answers in .cache/fixtures by default (NT_FIXTURE_MODE=replay), so it makes no
//! network requests; set NT_FIXTURES / NT_FIXTURE_MODE to change that. quit and end of input
//! exit 0.
//!
//! Every screen ends with `Ready #<n>` and the prompt `nt> `, where n counts the commands
//! run; `page=<kind> query=<q>` and `Back: <depth>` describe the current page.
use nt_core::{Client, CommentPage, Continuation, Page, Video};
use std::fmt::Write as _;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

const HELP: &str = "home [topic] | search <query> | more | open <n> | watch <id>\ncomments | replies <n> | channel <id> | back | state | quit";

#[derive(Clone, Copy)]
enum Kind {
    Home,
    Search,
    Watch,
    Channel,
    Comments,
    Replies,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Search => "search",
            Self::Watch => "watch",
            Self::Channel => "channel",
            Self::Comments => "comments",
            Self::Replies => "replies",
        }
    }
}

enum Content {
    Videos(Page<Video>),
    Comments(CommentPage),
    Unavailable(String),
}

struct Screen {
    kind: Kind,
    title: String,
    query: Option<String>,
    channel: Option<String>,
    comments: Option<Continuation>,
    content: Content,
}

impl Screen {
    fn new(kind: Kind, title: String, items: nt_core::Result<Page<Video>>) -> Self {
        Self {
            kind,
            title,
            query: None,
            channel: None,
            comments: None,
            content: items.map_or_else(
                |error| Content::Unavailable(error.to_string()),
                Content::Videos,
            ),
        }
    }

    fn home(client: &Client, topic: &str) -> Self {
        Self::new(
            Kind::Home,
            format!("Home / {topic}"),
            client.home(topic).wait(),
        )
    }

    fn watch(client: &Client, id: &str) -> Self {
        match client.watch(id).wait() {
            Ok(watch) => {
                let mut screen = Self::new(Kind::Watch, watch.title, Ok(watch.related));
                screen.channel = Some(watch.channel.id).filter(|id| !id.is_empty());
                screen.comments = watch.comments;
                screen
            }
            Err(error) => Self::new(Kind::Watch, format!("Watch / {id}"), Err(error)),
        }
    }

    fn channel(client: &Client, id: &str) -> Self {
        match client.channel(id).wait() {
            Ok(channel) => Self::new(Kind::Channel, channel.name, Ok(channel.videos)),
            Err(error) => Self::new(Kind::Channel, format!("Channel / {id}"), Err(error)),
        }
    }

    fn state(&self) -> String {
        format!(
            "page={} query={}",
            self.kind.name(),
            self.query.as_deref().unwrap_or("")
        )
    }
}

struct Repl {
    client: Client,
    current: Screen,
    history: Vec<Screen>,
}

impl Repl {
    fn navigate(&mut self, next: Screen) {
        self.history
            .push(std::mem::replace(&mut self.current, next));
    }

    fn command(&mut self, line: &str) -> Option<String> {
        let (command, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let arg = arg.trim();
        let index = || arg.parse::<usize>().ok()?.checked_sub(1);
        let next = match command {
            "home" => Screen::home(&self.client, if arg.is_empty() { "all" } else { arg }),
            "search" if !arg.is_empty() => {
                let mut screen = Screen::new(
                    Kind::Search,
                    format!("Search / {arg}"),
                    self.client.search(arg).wait(),
                );
                screen.query = Some(arg.to_owned());
                screen
            }
            "open" => {
                if let Content::Videos(page) = &self.current.content
                    && let Some(video) = index().and_then(|i| page.items.get(i))
                {
                    Screen::watch(&self.client, &video.id)
                } else {
                    return Some("Choose a numbered video on the current screen.".into());
                }
            }
            "watch" if !arg.is_empty() => Screen::watch(&self.client, arg),
            "channel" if !arg.is_empty() => Screen::channel(&self.client, arg),
            "comments" if arg.is_empty() => {
                let Some(cursor) = &self.current.comments else {
                    return Some("No comments are available on this page.".into());
                };
                let mut screen =
                    Screen::new(Kind::Comments, "Comments".into(), Ok(Page::default()));
                screen.content = self.client.more_comments(cursor).wait().map_or_else(
                    |error| Content::Unavailable(error.to_string()),
                    Content::Comments,
                );
                screen
            }
            "replies" => {
                let Content::Comments(page) = &self.current.content else {
                    return Some("Choose a numbered comment on the current screen.".into());
                };
                let Some(comment) = index().and_then(|i| page.items.get(i)) else {
                    return Some("Choose a numbered comment on the current screen.".into());
                };
                let Some(cursor) = comment.replies.as_ref() else {
                    return Some("No replies are available for this comment.".into());
                };
                let mut screen = Screen::new(
                    Kind::Replies,
                    format!("Replies / {}", comment.author),
                    Ok(Page::default()),
                );
                screen.content = self.client.more_replies(cursor).wait().map_or_else(
                    |error| Content::Unavailable(error.to_string()),
                    Content::Comments,
                );
                screen
            }
            "more" if arg.is_empty() => {
                return match &mut self.current.content {
                    Content::Videos(page) => {
                        let Some(cursor) = &page.next else {
                            return Some("There are no more videos.".into());
                        };
                        match self.client.more_videos(cursor).wait() {
                            Ok(more) => {
                                page.items.extend(more.items);
                                page.next = more.next;
                                None
                            }
                            Err(error) => Some(error.to_string()),
                        }
                    }
                    Content::Comments(page) => {
                        let Some(cursor) = &page.next else {
                            return Some("There are no more comments.".into());
                        };
                        let next = if matches!(self.current.kind, Kind::Replies) {
                            self.client.more_replies(cursor)
                        } else {
                            self.client.more_comments(cursor)
                        };
                        match next.wait() {
                            Ok(more) => {
                                page.items.extend(more.items);
                                page.next = more.next;
                                None
                            }
                            Err(error) => Some(error.to_string()),
                        }
                    }
                    Content::Unavailable(_) => Some("There is no list on this page.".into()),
                };
            }
            "back" if arg.is_empty() => {
                if let Some(previous) = self.history.pop() {
                    self.current = previous;
                    return None;
                }
                return Some("No previous page.".into());
            }
            "state" if arg.is_empty() => return Some(format!("State {}", self.current.state())),
            _ => return Some(HELP.into()),
        };
        self.navigate(next);
        None
    }

    fn observation_before(&self, command: &str) -> Option<String> {
        match command {
            "back" => self.history.last().map(|page| {
                format!(
                    "Before back: {} {}",
                    self.history.len(),
                    safe_line(&page.title, 64)
                )
            }),
            "state" => self
                .current
                .query
                .as_ref()
                .map(|query| format!("Before state: {}", safe_line(query, 62))),
            _ => None,
        }
    }

    fn render(
        &self,
        serial: u64,
        command: &str,
        before: Option<&str>,
        notice: Option<&str>,
    ) -> String {
        let first_line = |s: &str| safe_line(s, 64);
        let mut screen = format!("Page: {}\n", first_line(&self.current.title));
        if let Some(previous) = self.history.last() {
            let _ = writeln!(screen, "Previous: {}", first_line(&previous.title));
        }
        let _ = writeln!(screen, "{}", safe_line(&self.current.state(), 76));
        let _ = writeln!(screen, "Back: {}", self.history.len());
        if let Some(channel) = &self.current.channel {
            let _ = writeln!(screen, "Channel: {}", first_line(channel));
        }
        if self.current.comments.is_some() {
            screen.push_str("Comments available\n");
        }
        match &self.current.content {
            Content::Videos(page) => {
                list(&mut screen, &page.items, |video| &video.title);
                if page.next.is_some() {
                    screen.push_str("More available\n");
                }
            }
            Content::Comments(page) => {
                list(&mut screen, &page.items, |comment| &comment.text);
                if page.next.is_some() {
                    screen.push_str("More available\n");
                }
            }
            Content::Unavailable(error) => {
                let _ = writeln!(screen, "Error: {}", first_line(error));
            }
        }
        if let Some(notice) = notice {
            for line in notice.lines() {
                let _ = writeln!(screen, "{}", safe_line(line, 76));
            }
        }
        if let Some(before) = before {
            let _ = writeln!(screen, "{before}");
        }
        let _ = writeln!(screen, "Command: {}", safe_line(command, 70));
        let _ = write!(screen, "Ready #{serial}\nnt> ");
        screen
    }
}

fn list<T>(screen: &mut String, items: &[T], label: impl Fn(&T) -> &str) {
    if items.is_empty() {
        screen.push_str("No items\n");
    }
    let start = items.len().saturating_sub(8);
    for (index, item) in items.iter().enumerate().skip(start) {
        let _ = writeln!(screen, "{}. {}", index + 1, safe_line(label(item), 60));
    }
}

fn safe_line(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|character| {
            if character.is_control() || character == '\u{fffd}' {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn main() -> io::Result<()> {
    // SAFETY: this process has not initialized the core, the HTTP client, or any worker threads.
    unsafe {
        if std::env::var_os("NT_FIXTURE_MODE").is_none() {
            std::env::set_var("NT_FIXTURE_MODE", "replay");
        }
        if std::env::var_os("NT_FIXTURES").is_none_or(|path| path.is_empty()) {
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.cache/fixtures");
            std::env::set_var("NT_FIXTURES", fixtures);
        }
    }
    let client = Client::from_env();
    let mut repl = Repl {
        current: Screen::home(&client, "all"),
        client,
        history: Vec::new(),
    };
    let terminal = io::stdout().is_terminal();
    let mut output = io::stdout().lock();
    let mut input = io::stdin().lock();
    let mut serial = 0u64;
    write!(output, "{}", repl.render(serial, "", None, None))?;
    output.flush()?;
    let mut line = Vec::new();
    while input.read_until(b'\n', &mut line)? != 0 {
        let decoded = String::from_utf8_lossy(&line);
        let command = decoded.trim();
        if command == "quit" {
            return Ok(());
        }
        serial += 1;
        writeln!(output, "\nRunning #{serial}: {}", safe_line(command, 60))?;
        output.flush()?;
        let before = repl.observation_before(command);
        let notice = repl.command(command);
        if terminal {
            output.write_all(b"\x1b[2J\x1b[H")?;
        }
        write!(
            output,
            "{}",
            repl.render(serial, command, before.as_deref(), notice.as_deref())
        )?;
        output.flush()?;
        line.clear();
    }
    Ok(())
}
