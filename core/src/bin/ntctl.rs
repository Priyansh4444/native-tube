//! A command-line driver for nt-core, for checking the backend without any UI.
//!
//! ntctl home [topic] | search <query> | more <search query> | watch <id> | comments <id>
//!      | channel <id> | suggest <prefix> | subs | image <url> <out.png> [width]
//!      | play <id> <out.png> [seconds]

use nt_core::*;
use std::time::{Duration, Instant};

fn print_videos(page: &Page<Video>) {
    for (i, v) in page.items.iter().enumerate() {
        println!(
            "{i:>3}  {}  {:<60.60}  {:<24.24}  {:>8}  {}",
            v.id,
            v.title,
            v.channel,
            v.duration.as_deref().unwrap_or(if v.is_live { "LIVE" } else { "-" }),
            v.meta_line()
        );
    }
    println!("-- {} items, more: {}", page.items.len(), page.next.is_some());
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let c = client();
    let t = Instant::now();
    match arg(0).as_str() {
        "home" => print_videos(&c.home(&arg(1)).wait()?),
        "search" => print_videos(&c.search(&arg(1)).wait()?),
        "more" => {
            let first = c.search(&arg(1)).wait()?;
            let next = first.next.ok_or("no continuation")?;
            print_videos(&c.more_videos(&next).wait()?);
        }
        "watch" => {
            let w = c.watch(&arg(1)).wait()?;
            println!("{}\n{} | {} | {} likes", w.title, w.views.as_deref().unwrap_or("-"), w.date.as_deref().unwrap_or("-"), w.likes.as_deref().unwrap_or("-"));
            println!("channel: {} ({}) {} avatar={}", w.channel.name, w.channel.id, w.channel.subscribers.as_deref().unwrap_or("-"), w.channel.avatar.is_some());
            println!("description: {:.200}", w.description.replace('\n', " "));
            println!("comments cursor: {}", w.comments.is_some());
            println!("related:");
            print_videos(&w.related);
            if let Some(next) = &w.related.next {
                println!("related page 2:");
                print_videos(&c.more_videos(next).wait()?);
            }
        }
        "comments" => {
            let w = c.watch(&arg(1)).wait()?;
            let page = c.more_comments(w.comments.as_ref().ok_or("no comments")?).wait()?;
            println!("total: {:?}", page.total);
            for cm in &page.items {
                println!("{:<24.24} {:>6} likes {:>5} replies  {:.90}", cm.author, cm.likes.as_deref().unwrap_or("0"), cm.reply_count.as_deref().unwrap_or("0"), cm.text.replace('\n', " "));
            }
            if let Some(next) = &page.next {
                let p2 = c.more_comments(next).wait()?;
                println!("-- page 2: {} comments, more: {}", p2.items.len(), p2.next.is_some());
            }
        }
        "channel" => {
            let ch = c.channel(&arg(1)).wait()?;
            println!("{} {:?} {:?} {:?} banner={} avatar={:?}", ch.name, ch.handle, ch.subscribers, ch.video_count, ch.banner.is_some(), ch.avatar);
            print_videos(&ch.videos);
            if let Some(next) = &ch.videos.next {
                println!("page 2:");
                print_videos(&c.more_videos(next).wait()?);
            }
        }
        "suggest" => println!("{:?}", c.suggestions(&arg(1)).wait()?),
        "subs" => {
            let subs = library().subscriptions();
            println!("{} subscriptions", subs.len());
            for v in c.subscription_feed(subs).wait()? {
                println!("{}  {:<60.60}  {:<20.20}  {}", v.id, v.title, v.channel, v.meta_line());
            }
        }
        "image" => {
            let width = arg(3).parse().unwrap_or(0);
            let img = images::image(&arg(1), width).wait()?;
            image::save_buffer(arg(2), &img.pixels, img.width, img.height, image::ColorType::Rgba8)?;
            println!("{}x{}", img.width, img.height);
        }
        "play" => {
            let secs: f64 = arg(3).parse().unwrap_or(6.0);
            let player = Player::new(PixelFormat::Rgba, || {})?;
            player.load(&arg(1))?;
            let deadline = Instant::now() + Duration::from_secs_f64(secs + 30.0);
            let mut started = None;
            while Instant::now() < deadline {
                let s = player.state();
                if let Status::Failed(e) = &s.status {
                    return Err(e.clone().into());
                }
                if started.is_none() && player.frame().is_some() {
                    started = Some(Instant::now());
                    println!("first frame after {:.2}s: {:?}", t.elapsed().as_secs_f64(), player.frame());
                }
                if let Some(at) = started
                    && at.elapsed().as_secs_f64() >= secs
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let s = player.state();
            println!("state: {:?} pos={:.1}/{:.1} paused={} size={:?} buffered={:.1}s", s.status, s.position, s.duration, s.paused, s.video_size, s.buffered_ahead);
            let st = player.render_stats();
            println!("rendered {} frames, avg {:.2} ms, max {:.2} ms", st.frames, st.total_render_ms / st.frames.max(1) as f64, st.max_render_ms);
            let f = player.frame().ok_or("no frame")?;
            image::save_buffer(arg(2), f.pixels(), f.width, f.height, image::ColorType::Rgba8)?;
        }
        _ => {
            eprintln!("usage: ntctl home [topic] | search <q> | more <q> | watch <id> | comments <id> | channel <id> | suggest <p> | subs | image <url> <out.png> [w] | play <id> <out.png> [secs]");
            std::process::exit(2);
        }
    }
    eprintln!("({:.0} ms)", t.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
