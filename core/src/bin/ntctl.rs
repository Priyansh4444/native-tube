//! A command-line driver for nt-core, for checking the backend without any UI.
//!
//! ntctl home [topic] | search <query> | more <search query> | watch <id> | comments <id>
//!      | channel <id> | suggest <prefix> | subs | image <url> <out.png> [width]
//!      | resolve <id> | play <id> <out.png> [seconds] [seek-to-seconds]
//!      | thumbs <topic> <physical-width> [output-dir]

use nt_core::*;
use std::time::{Duration, Instant};

fn print_videos(page: &Page<Video>) {
    for (i, v) in page.items.iter().enumerate() {
        println!(
            "{i:>3}  {}  {:<60.60}  {:<24.24}{} {:>8}  {}{}{}",
            v.id,
            v.title,
            v.channel,
            if v.channel_verified { "✓" } else { " " },
            v.duration
                .as_deref()
                .unwrap_or(if v.is_live { "LIVE" } else { "-" }),
            v.meta_line(),
            if v.badges.is_empty() {
                String::new()
            } else {
                format!("  [{}]", v.badges.join(","))
            },
            v.upcoming
                .as_deref()
                .map(|u| format!("  ({u})"))
                .unwrap_or_default()
        );
    }
    println!(
        "-- {} items, more: {}",
        page.items.len(),
        page.next.is_some()
    );
}

fn print_search_item(item: &SearchResult, depth: usize) {
    let pad = "  ".repeat(depth);
    match item {
        SearchResult::Video(v) => println!("{pad}video {} {}", v.id, v.title),
        SearchResult::Playlist(p) => {
            println!("{pad}playlist {} {} {:?}", p.id, p.title, p.video_count)
        }
        SearchResult::Channel(c) => println!(
            "{pad}channel {} {} {:?} {:?}",
            c.channel.id, c.channel.name, c.handle, c.channel.subscribers
        ),
        SearchResult::Shelf(s) => {
            println!("{pad}shelf {}", s.title);
            for item in &s.items {
                print_search_item(item, depth + 1);
            }
        }
    }
}

fn first_frame(player: &Player, id: &str) -> std::result::Result<f64, Box<dyn std::error::Error>> {
    let start = Instant::now();
    player.load(id)?;
    while start.elapsed() < Duration::from_secs(30) {
        if let Status::Failed(e) = player.state().status {
            return Err(e.into());
        }
        if player.frame().is_some() {
            return Ok(start.elapsed().as_secs_f64() * 1000.0);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("no video frame within 30 seconds".into())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // A command-line test tool never makes sound unless asked to (NT_MUTE=0).
    if std::env::var_os("NT_MUTE").is_none() {
        // SAFETY: set before any other thread exists.
        unsafe { std::env::set_var("NT_MUTE", "1") };
    }
    if std::env::var_os("NT_NO_MPRIS").is_none() {
        // SAFETY: as above. A test tool stays out of the desktop's media controls.
        unsafe { std::env::set_var("NT_NO_MPRIS", "1") };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let c = client();
    let t = Instant::now();
    match arg(0).as_str() {
        "next-test" => {
            let (first, next) = (arg(1), arg(2));
            let player = Player::new(PixelFormat::Rgba, || {})?;
            first_frame(&player, &first)?;
            let cold = first_frame(&player, &next)?;
            first_frame(&player, &first)?;
            player.prepare_next(Some(&next));
            std::thread::sleep(Duration::from_secs(1));
            let prepared = first_frame(&player, &next)?;
            println!(
                "natural successor: cold resolve + first frame {cold:.0} ms; prepared first frame {prepared:.0} ms"
            );
        }
        "preview" => {
            let player = Player::new_preview(PixelFormat::Rgba, || {})?;
            let rss_before = metrics::rss_bytes();
            let cpu = metrics::cpu_seconds();
            let start = first_frame(&player, &arg(1))?;
            std::thread::sleep(Duration::from_secs(5));
            let frame = player.frame().ok_or("no preview frame")?;
            let state = player.state();
            println!(
                "{}x{} muted={} position={:.1} first_frame={start:.0} ms cpu={:.2}s RSS_delta={:.1} MiB",
                frame.width,
                frame.height,
                state.muted,
                state.position,
                metrics::cpu_seconds() - cpu,
                (metrics::rss_bytes() as f64 - rss_before as f64) / (1024.0 * 1024.0)
            );
            image::save_buffer(
                arg(2),
                frame.pixels(),
                frame.width,
                frame.height,
                image::ColorType::Rgba8,
            )?;
        }
        "home-feed" => {
            let feed = c.home_feed().wait()?;
            println!(
                "chips: {:?}",
                feed.chips.iter().map(|c| &c.label).collect::<Vec<_>>()
            );
            if feed.chips.is_empty() {
                println!(
                    "fixed topics: {:?}",
                    TOPICS.iter().map(|t| t.title).collect::<Vec<_>>()
                );
            }
            print_videos(&feed.videos);
        }
        "queue" => {
            let library = library();
            match arg(1).as_str() {
                "add" => {
                    let id = arg(2);
                    library.enqueue(&Video {
                        thumbnail: Video::thumbnail_url(&id),
                        id,
                        ..Video::default()
                    });
                }
                "remove" => library.remove_queued(&arg(2)),
                "hide-video" => library.set_video_hidden(&arg(2), true),
                "hide-channel" => library.set_channel_hidden(&arg(2), true),
                _ => {}
            }
            library.flush();
            println!(
                "{:?}",
                library.queue().iter().map(|v| &v.id).collect::<Vec<_>>()
            );
        }
        "last-played" => println!("{:?}", library().last_played()),
        "home" => print_videos(&c.home(&arg(1)).wait()?),
        "search" => {
            let result = c.search_page(&arg(1)).wait()?;
            for notice in &result.notices {
                match notice {
                    SearchNotice::AgeGated { title, body } => println!("{title}\n{body}"),
                    SearchNotice::NoResults => println!("No YouTube video results found"),
                    SearchNotice::Corrected { showing, original } => {
                        println!("Showing results for {showing}. Search instead for {original}.")
                    }
                    SearchNotice::DidYouMean { suggestion } => {
                        println!("Did you mean {suggestion}?")
                    }
                }
            }
            print_videos(&result.results);
            if !result.transcripts.items.is_empty() {
                println!(
                    "Matches in video transcripts · search.pronsh.dev ({:?} total)",
                    result.transcript_total
                );
                for v in &result.transcripts.items {
                    if let Some(moment) = &v.moment {
                        println!(
                            "{} {} [{}] {} · {} — {}",
                            v.id,
                            v.title,
                            moment.label,
                            v.channel,
                            v.meta_line(),
                            moment.snippet
                        );
                    } else {
                        println!("{} {} — {}", v.id, v.title, v.channel);
                    }
                }
                println!(
                    "-- {} matches, more: {}",
                    result.transcripts.items.len(),
                    result.transcripts.next.is_some()
                );
            }
        }
        "search-filter" => {
            let q = arg(1);
            let requested = arg(2);
            let page = if requested.eq_ignore_ascii_case("all") || requested.is_empty() {
                c.search_filtered(&q, &SearchFilter::All).wait()?
            } else {
                let initial = c.search_filtered(&q, &SearchFilter::All).wait()?;
                let filter = initial
                    .chips
                    .iter()
                    .chain(initial.filters.iter().flat_map(|g| &g.options))
                    .find(|option| option.label.eq_ignore_ascii_case(&requested))
                    .and_then(|option| option.filter.clone())
                    .unwrap_or(SearchFilter::Params(requested));
                c.search_filtered(&q, &filter).wait()?
            };
            for item in &page.entries {
                print_search_item(item, 0);
            }
            for chip in &page.chips {
                println!(
                    "chip {}{}",
                    chip.label,
                    if chip.selected { " [selected]" } else { "" }
                );
            }
            for group in &page.filters {
                let options: Vec<_> = group
                    .options
                    .iter()
                    .map(|option| option.label.as_str())
                    .collect();
                println!("{}: {}", group.title, options.join(", "));
            }
            for v in &page.transcripts.items {
                if let Some(m) = &v.moment {
                    println!("{} {} [{}] {}", v.id, v.title, m.label, m.snippet);
                }
            }
            for notice in page.notices {
                println!("{notice:?}");
            }
        }
        "more" => {
            let first = c.search(&arg(1)).wait()?;
            let next = first.next.ok_or("no continuation")?;
            print_videos(&c.more_videos(&next).wait()?);
        }
        "watch" => {
            let w = c.watch(&arg(1)).wait()?;
            println!(
                "{}\n{} | {} | {} likes",
                w.title,
                w.views.as_deref().unwrap_or("-"),
                w.date.as_deref().unwrap_or("-"),
                w.likes.as_deref().unwrap_or("-")
            );
            println!(
                "channel: {} ({}) {} avatar={} verified={}",
                w.channel.name,
                w.channel.id,
                w.channel.subscribers.as_deref().unwrap_or("-"),
                w.channel.avatar.is_some(),
                w.channel_verified
            );
            let chips: Vec<String> = w
                .related_chips
                .iter()
                .map(|c| format!("{}{}", c.label, if c.selected { "*" } else { "" }))
                .collect();
            println!("related chips: {}", chips.join(" | "));
            println!("description: {:.200}", w.description.replace('\n', " "));
            println!("comments cursor: {}", w.comments.is_some());
            for ch in &w.chapters {
                println!(
                    "chapter {:.3} s: {} (thumbnail={})",
                    ch.start,
                    ch.title,
                    ch.thumbnail.is_some()
                );
            }
            println!("heatmap: {} samples", w.heatmap.len());
            println!("related:");
            print_videos(&w.related);
            if let Some(next) = &w.related.next {
                println!("related page 2:");
                print_videos(&c.more_videos(next).wait()?);
            }
            if let Some(chip) = w.related_chips.iter().find(|c| !c.selected)
                && let Some(next) = &chip.next
            {
                println!("related chip {}:", chip.label);
                print_videos(&c.more_videos(next).wait()?);
            }
        }
        "comments" | "replies" => {
            // ntctl comments <id> [sort label, e.g. Newest] | ntctl replies <id>
            let w = c.watch(&arg(1)).wait()?;
            let mut page = c
                .more_comments(w.comments.as_ref().ok_or("no comments")?)
                .wait()?;
            let sorts: Vec<String> = page
                .sort
                .iter()
                .map(|s| format!("{}{}", s.label, if s.selected { "*" } else { "" }))
                .collect();
            println!("total: {:?}  sort: {}", page.total, sorts.join(" | "));
            if arg(0) == "comments" && !arg(2).is_empty() {
                let sort = page
                    .sort
                    .iter()
                    .find(|s| s.label.eq_ignore_ascii_case(&arg(2)))
                    .cloned()
                    .ok_or("no such sort order")?;
                page = c.more_comments(&sort.next).wait()?;
                println!("-- sorted by {}", sort.label);
            }
            let print = |cm: &Comment, indent: &str| {
                let flags = [
                    (cm.pinned, "pinned"),
                    (cm.hearted, "hearted"),
                    (cm.by_creator, "creator"),
                    (cm.author_verified, "verified"),
                ]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(",");
                println!(
                    "{indent}{:<24.24} {:>6} likes {:>5} replies {:<10.10} [{flags}] {:.70}",
                    cm.author,
                    cm.likes.as_deref().unwrap_or("0"),
                    cm.reply_count.as_deref().unwrap_or("0"),
                    cm.published.as_deref().unwrap_or("-"),
                    cm.text.replace('\n', " ")
                );
            };
            for cm in &page.items {
                print(cm, "");
            }
            if arg(0) == "replies" {
                let parent = page
                    .items
                    .iter()
                    .filter(|cm| cm.replies.is_some())
                    .max_by_key(|cm| {
                        cm.reply_count
                            .as_deref()
                            .and_then(|n| n.replace(',', "").parse::<u64>().ok())
                            .unwrap_or(0)
                    })
                    .ok_or("no comment with replies")?;
                println!(
                    "-- replies to {} ({:?}):",
                    parent.author, parent.reply_count
                );
                let replies = c
                    .more_replies(parent.replies.as_ref().ok_or("no reply cursor")?)
                    .wait()?;
                for cm in &replies.items {
                    print(cm, "    ");
                }
                println!(
                    "-- {} replies, more: {}",
                    replies.items.len(),
                    replies.next.is_some()
                );
                if let Some(next) = &replies.next {
                    let more = c.more_replies(next).wait()?;
                    println!(
                        "-- replies page 2: {} replies, more: {}",
                        more.items.len(),
                        more.next.is_some()
                    );
                }
            } else if let Some(next) = &page.next {
                let p2 = c.more_comments(next).wait()?;
                println!(
                    "-- page 2: {} comments, more: {}",
                    p2.items.len(),
                    p2.next.is_some()
                );
            }
        }
        "channel" | "channel-tab" => {
            let tab = match arg(2).to_ascii_lowercase().as_str() {
                "" | "videos" => ChannelTab::Videos,
                "home" => ChannelTab::Home,
                "live" => ChannelTab::Live,
                "playlists" => ChannelTab::Playlists,
                _ => return Err("unknown channel tab".into()),
            };
            let ch = c.channel_tab(&arg(1), tab).wait()?;
            println!(
                "{} {:?} {:?} {:?} banner={} avatar={:?}",
                ch.name,
                ch.handle,
                ch.subscribers,
                ch.video_count,
                ch.banner.is_some(),
                ch.avatar
            );
            println!(
                "tabs {:?}, selected {:?}, verified={}",
                ch.tabs, ch.selected_tab, ch.verified
            );
            println!(
                "About preview: {}  links: {:?}",
                ch.description, ch.links_label
            );
            for link in &ch.links {
                println!("{} {}", link.label, link.url);
            }
            if let Some(about) = ch
                .about
                .as_ref()
                .filter(|_| args.iter().any(|s| s == "--about"))
            {
                let about = c.channel_about(about).wait()?;
                println!("About: {}", about.description);
                for link in &about.links {
                    println!("{} {}", link.label, link.url);
                }
            }
            if let Some(featured) = &ch.featured {
                println!("Featured: {} {}", featured.id, featured.title);
            }
            for sort in &ch.video_sort {
                println!(
                    "sort {}{}",
                    sort.label,
                    if sort.selected { " [selected]" } else { "" }
                );
            }
            for playlist in &ch.playlists.items {
                print_search_item(&SearchResult::Playlist(playlist.clone()), 0);
            }
            if let Some(sort) = ch
                .video_sort
                .iter()
                .find(|s| s.label.eq_ignore_ascii_case(&arg(3)))
                .and_then(|s| s.next.as_ref())
            {
                print_videos(&c.more_videos(sort).wait()?);
            } else {
                print_videos(&ch.videos);
                if arg(0) == "channel"
                    && let Some(next) = &ch.videos.next
                {
                    println!("page 2:");
                    print_videos(&c.more_videos(next).wait()?);
                }
            }
        }
        "playlist" => {
            let id = arg(1);
            let panel = if arg(2).is_empty() {
                c.playlist(&id).wait()?
            } else {
                c.watch_in_playlist(&arg(2), &id)
                    .wait()?
                    .playlist
                    .ok_or("no playlist panel")?
            };
            println!(
                "{} {} selected={:?} mix={}",
                panel.id, panel.title, panel.index, panel.is_mix
            );
            println!(
                "previous={:?}, next={:?}",
                panel.previous_video().map(|v| &v.id),
                panel.next_video().map(|v| &v.id)
            );
            print_videos(&Page {
                items: panel.items,
                next: panel.next,
            });
        }
        "captions" => {
            let s = try_runtime()?.block_on(resolve::resolve(&arg(1), 360))?;
            for (i, track) in s.captions.iter().enumerate() {
                println!(
                    "{i}: {} [{}], ASR={}, translatable={}",
                    track.label, track.language_code, track.auto_generated, track.translatable
                );
            }
            println!(
                "{} translation languages; /player {:.0} ms",
                s.caption_languages.len(),
                s.player_ms
            );
        }
        "resolve" => {
            let s = try_runtime()?
                .block_on(resolve::resolve(&arg(1), resolve::default_max_height()))?;
            println!(
                "video: {}  audio: {}  live: {}",
                s.video_desc, s.audio_desc, s.is_live
            );
            println!(
                "visitor id {:.0} ms, /player {:.0} ms",
                s.visitor_ms, s.player_ms
            );
            if arg(2) == "--json" {
                println!(
                    "{}",
                    serde_json::json!({ "video": s.video, "audio": s.audio, "user_agent": resolve::USER_AGENT })
                );
            }
        }
        "serve" => {
            // A benchmark driver: reads "open <id>" lines on stdin, prints JSON events on stdout.
            // The first `open` creates the player (as an app does on its first video).
            use std::io::BufRead;
            let epoch = || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64() * 1000.0)
                    .unwrap_or(0.0)
            };
            let mut player: Option<Player> = None;
            println!("{{\"event\":\"ready\",\"epoch_ms\":{:.1}}}", epoch());
            for line in std::io::stdin().lock().lines() {
                let line = line?;
                let mut parts = line.split_whitespace();
                match (parts.next(), parts.next()) {
                    (Some("open"), Some(id)) => {
                        let t0 = Instant::now();
                        let cold = player.is_none();
                        let p = match &player {
                            Some(p) => p,
                            None => player.insert(Player::new(PixelFormat::Rgba, || {})?),
                        };
                        p.load(id)?;
                        let deadline = Instant::now() + Duration::from_secs(45);
                        let mut failed = None;
                        while p.frame().is_none() && Instant::now() < deadline {
                            if let Status::Failed(e) = p.state().status {
                                failed = Some(e);
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        let ms = t0.elapsed().as_secs_f64() * 1000.0;
                        let f = p.frame();
                        println!(
                            "{{\"event\":\"first_frame\",\"id\":\"{id}\",\"cold\":{cold},\"ms\":{ms:.1},\"ok\":{},\"error\":{:?},\"size\":\"{}\",\"epoch_ms\":{:.1}}}",
                            f.is_some() && failed.is_none(),
                            failed.unwrap_or_default(),
                            f.map(|f| format!("{}x{}", f.width, f.height))
                                .unwrap_or_default(),
                            epoch()
                        );
                    }
                    (Some("stats"), _) => {
                        let st = player
                            .as_ref()
                            .map(|p| p.render_stats())
                            .unwrap_or_default();
                        let s = player.as_ref().map(|p| p.state()).unwrap_or_default();
                        println!(
                            "{{\"event\":\"stats\",\"frames\":{},\"render_ms_total\":{:.1},\"position\":{:.2},\"muted\":{},\"buffering\":{}}}",
                            st.frames, st.total_render_ms, s.position, s.muted, s.buffering
                        );
                    }
                    (Some("stop"), _) => {
                        if let Some(p) = &player {
                            p.stop();
                        }
                        println!("{{\"event\":\"stopped\"}}");
                    }
                    (Some("quit"), _) => break,
                    _ => {}
                }
                use std::io::Write;
                std::io::stdout().flush()?;
            }
        }
        "storyboard" => {
            // ntctl storyboard <id> <seconds> <out.png> [drawn width px]
            let s = try_runtime()?.block_on(resolve::resolve(&arg(1), 360))?;
            let sb = s.storyboard.ok_or("this video has no storyboard")?;
            for l in sb.levels() {
                println!(
                    "level {}x{}: {} frames, {}x{} a sheet, every {:.2} s",
                    l.width(),
                    l.height(),
                    l.count(),
                    l.columns(),
                    l.rows(),
                    l.interval()
                );
            }
            let secs: f64 = arg(2).parse().unwrap_or(0.0);
            let width = arg(4).parse().unwrap_or(160);
            let frame = sb.level_for_width(width).frame_at(secs);
            println!(
                "{secs:.1} s -> {}x{} at ({}, {}) of {}",
                frame.width, frame.height, frame.x, frame.y, frame.sheet_url
            );
            let at = Instant::now();
            let img = sb.preview(secs, width).wait()?;
            println!(
                "preview {}x{} in {:.0} ms (sheet fetched and cropped)",
                img.width,
                img.height,
                at.elapsed().as_secs_f64() * 1000.0
            );
            let at = Instant::now();
            let again = sb
                .preview(secs + sb.level_for_width(width).interval(), width)
                .wait()?;
            println!(
                "next frame {}x{} in {:.1} ms (sheet from the image cache)",
                again.width,
                again.height,
                at.elapsed().as_secs_f64() * 1000.0
            );
            image::save_buffer(
                arg(3),
                &img.pixels,
                img.width,
                img.height,
                image::ColorType::Rgba8,
            )?;
        }
        "audio" => {
            // ntctl audio <id> [track id] [out.png]: list audio languages, optionally switch to one
            // three seconds in and report how long the switch took and where playback was.
            let player = Player::new(PixelFormat::Rgba, || {})?;
            player.load(&arg(1))?;
            let deadline = Instant::now() + Duration::from_secs(30);
            while player.frame().is_none() && Instant::now() < deadline {
                if let Status::Failed(e) = player.state().status {
                    return Err(e.into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let s = player.state();
            for t in &s.audio_tracks {
                println!(
                    "{:<12} {:<28} original={} auto-dubbed={}{}",
                    t.id,
                    t.label,
                    t.original,
                    t.auto_dubbed,
                    if s.audio_track.as_deref() == Some(t.id.as_str()) {
                        "  <- playing"
                    } else {
                        ""
                    }
                );
            }
            if !arg(2).is_empty() {
                std::thread::sleep(Duration::from_secs(3));
                let before = player.state().position;
                let at = Instant::now();
                player.set_audio_track(&arg(2)).wait()?;
                let ms = at.elapsed().as_secs_f64() * 1000.0;
                let after = player.state();
                println!(
                    "switched to {:?} in {ms:.0} ms; position {before:.2} s -> {:.2} s",
                    after.audio_track, after.position
                );
                std::thread::sleep(Duration::from_secs(2));
                let later = player.state();
                println!(
                    "2 s later: position {:.2} s, status {:?}, buffering {}",
                    later.position, later.status, later.buffering
                );
                if arg(3) == "--race" {
                    // Back to the original, then at once a quality change and a stop: neither
                    // may wait for the stream to open, and the reload must keep the choice.
                    let original = s
                        .audio_tracks
                        .first()
                        .map(|t| t.id.clone())
                        .unwrap_or_default();
                    let switch = player.set_audio_track(&original);
                    let at = Instant::now();
                    player.set_quality(Some(360));
                    println!(
                        "quality change during the switch returned in {:.2} ms",
                        at.elapsed().as_secs_f64() * 1000.0
                    );
                    println!("switch result: {:?}", switch.wait());
                    let deadline = Instant::now() + Duration::from_secs(20);
                    while (player.state().height != 360 || player.state().status != Status::Ready)
                        && Instant::now() < deadline
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let st = player.state();
                    println!(
                        "after the reload: audio {:?}, height {}, position {:.2} s",
                        st.audio_track, st.height, st.position
                    );
                    let other = s
                        .audio_tracks
                        .last()
                        .map(|t| t.id.clone())
                        .unwrap_or_default();
                    let _pending = player.set_audio_track(&other);
                    let at = Instant::now();
                    player.stop();
                    println!(
                        "stop during a switch returned in {:.2} ms",
                        at.elapsed().as_secs_f64() * 1000.0
                    );
                } else if !arg(3).is_empty() {
                    let f = player.frame().ok_or("no frame")?;
                    image::save_buffer(
                        arg(3),
                        f.pixels(),
                        f.width,
                        f.height,
                        image::ColorType::Rgba8,
                    )?;
                }
            }
        }
        "suggest" => println!("{:?}", c.suggestions(&arg(1)).wait()?),
        "subs" => {
            let subs = library().subscriptions();
            println!("{} subscriptions", subs.len());
            for v in c.subscription_feed(subs).wait()? {
                println!(
                    "{}  {:<60.60}  {:<20.20}  {}",
                    v.id,
                    v.title,
                    v.channel,
                    v.meta_line()
                );
            }
        }
        "thumbs" => {
            // ntctl thumbs <topic> <drawn width px> [out dir]: every poster of a home feed at
            // one card width, all requested at once as a fast scroll does, with the CPU used.
            let page = c.home(&arg(1)).wait()?;
            let width: u32 = arg(2).parse().unwrap_or(384);
            let cpu = metrics::cpu_seconds();
            let at = Instant::now();
            let jobs: Vec<_> = page
                .items
                .iter()
                .map(|v| (v.id.clone(), images::image(&v.thumbnail, width)))
                .collect();
            let (mut ok, mut failed) = (0, 0);
            let mut sizes = std::collections::BTreeMap::new();
            for (id, job) in jobs {
                match job.wait() {
                    Ok(img) => {
                        ok += 1;
                        *sizes.entry((img.width, img.height)).or_insert(0) += 1;
                        if !arg(3).is_empty() {
                            image::save_buffer(
                                format!("{}/{id}.png", arg(3)),
                                &img.pixels,
                                img.width,
                                img.height,
                                image::ColorType::Rgba8,
                            )?;
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("{id}: {e}");
                    }
                }
            }
            println!(
                "{ok} posters ({failed} failed) at {width} px in {:.0} ms wall, {:.2} s CPU; sizes {sizes:?}",
                at.elapsed().as_secs_f64() * 1000.0,
                metrics::cpu_seconds() - cpu
            );
        }
        "image" => {
            let width = arg(3).parse().unwrap_or(0);
            let img = images::image(&arg(1), width).wait()?;
            image::save_buffer(
                arg(2),
                &img.pixels,
                img.width,
                img.height,
                image::ColorType::Rgba8,
            )?;
            println!("{}x{}", img.width, img.height);
        }
        "play" => {
            let secs: f64 = arg(3).parse().unwrap_or(6.0);
            let seek: Option<f64> = args.get(4).and_then(|s| s.parse().ok());
            let player = Player::new(PixelFormat::Rgba, || {})?;
            let start = args
                .windows(2)
                .find(|pair| pair[0] == "--start")
                .and_then(|pair| pair[1].parse().ok())
                .unwrap_or(0.0);
            player.load_at(&arg(1), start)?;
            let captions = args
                .windows(2)
                .find(|pair| pair[0] == "--captions")
                .map(|pair| {
                    pair[1]
                        .split_once(':')
                        .map_or((pair[1].as_str(), None), |(track, translation)| {
                            (track, Some(translation.to_owned()))
                        })
                });
            let flag = |name: &str| args.iter().any(|a| a == name);
            let mut toggles_applied = false;
            let mut captions_applied = false;
            let mut seeked = seek.is_none();
            let deadline = Instant::now() + Duration::from_secs_f64(secs + 30.0);
            let mut started = None;
            while Instant::now() < deadline {
                let s = player.state();
                if let Status::Failed(e) = &s.status {
                    return Err(e.clone().into());
                }
                if started.is_none() && player.frame().is_some() {
                    started = Some(Instant::now());
                    println!(
                        "first frame after {:.2}s: {:?}",
                        t.elapsed().as_secs_f64(),
                        player.frame()
                    );
                }
                if !toggles_applied && player.frame().is_some() {
                    if flag("--loop") {
                        player.set_loop(true);
                    }
                    if flag("--normalize") {
                        player.set_normalize(true);
                    }
                    let s = player.state();
                    println!(
                        "loop={} normalize={} supported={} gain={:.2} dB",
                        s.looping, s.normalize, s.normalization_supported, s.normalization_gain_db
                    );
                    toggles_applied = true;
                }
                if let Some((code, translation)) = &captions
                    && !captions_applied
                    && player.frame().is_some()
                {
                    let index = s
                        .captions
                        .iter()
                        .position(|track| &track.language_code == code)
                        .ok_or("no caption track in that language")?;
                    let at = Instant::now();
                    player
                        .set_captions(Some((index, translation.clone())))
                        .wait()?;
                    println!(
                        "captions loaded in {:.0} ms",
                        at.elapsed().as_secs_f64() * 1000.0
                    );
                    captions_applied = true;
                    started = Some(Instant::now());
                }
                if !seeked
                    && started.is_some()
                    && let Some(to) = seek
                {
                    let at = Instant::now();
                    player.seek(to);
                    let before = player.frame_generation();
                    while player.state().position < to - 1.0
                        || player.frame_generation() <= before + 2
                    {
                        std::thread::sleep(Duration::from_millis(20));
                        if at.elapsed() > Duration::from_secs(20) {
                            break;
                        }
                    }
                    println!(
                        "seek to {to:.0}s took {:.2}s (now at {:.1}s)",
                        at.elapsed().as_secs_f64(),
                        player.state().position
                    );
                    seeked = true;
                    started = Some(Instant::now());
                }
                if let Some(at) = started
                    && at.elapsed().as_secs_f64() >= secs
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let s = player.state();
            println!(
                "state: {:?} pos={:.1}/{:.1} paused={} muted={} size={:?} buffered={:.1}s",
                s.status, s.position, s.duration, s.paused, s.muted, s.video_size, s.buffered_ahead
            );
            let st = player.render_stats();
            println!(
                "rendered {} frames, avg {:.2} ms, max {:.2} ms",
                st.frames,
                st.total_render_ms / st.frames.max(1) as f64,
                st.max_render_ms
            );
            let f = player.frame().ok_or("no frame")?;
            image::save_buffer(
                arg(2),
                f.pixels(),
                f.width,
                f.height,
                image::ColorType::Rgba8,
            )?;
        }
        _ => {
            eprintln!(
                "usage: ntctl home [topic] | home-feed | search <q> | search-filter <q> <filter> | more <q> | watch <id> | comments <id> | replies <id> | channel-tab <id> <tab> | playlist <id> [video] | captions <id> | audio <id> [track] | storyboard <id> <secs> <out.png> | image <url> <out.png> [w] | play <id> <out.png> [secs] [--captions en] | preview <id> <out.png> | queue [add|remove|hide-video|hide-channel] [id] | next-test <id> <next> | last-played"
            );
            std::process::exit(2);
        }
    }
    eprintln!("({:.0} ms)", t.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
