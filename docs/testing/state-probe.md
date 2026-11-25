# App state probe (contract)

Owner: the coordinator. The app agents implement it and `tools/nt-explore` reads it. Change it
only through the coordinator.

The probe is what Bombadil's "extractors" are for a web page: it lets a property-based explorer
see the app's state and pick valid actions. Bombadil itself only drives browsers and terminals,
so the native apps expose their state this way instead.

## Output

When `NT_PROBE=1` is set, the app writes one line to **stderr** for each state change. Changes
are coalesced to at most one line per 100 ms, and a line is also written every 1 s while
nothing changes, as a heartbeat:

```
NT_STATE {"t_ms":1234,"page":"home",...}
```

The JSON object is a single line with these fields. Use `null` for anything that doesn't apply.

| Field | Type | Meaning |
|---|---|---|
| `t_ms` | number | milliseconds since start |
| `page` | string | `home`, `search`, `watch`, `channel`, `subscriptions`, `history`, `watch_later` |
| `query` | string \| null | text currently in the search field |
| `results_query` | string \| null | the query the shown search results are for (search page only) |
| `topic` | string \| null | selected chip on Home |
| `scroll_y` | number | main content scroll offset, logical px |
| `max_scroll` | number | largest valid `scroll_y` |
| `items` | number | content items loaded on the page (cards, rows, comments) |
| `loading` | bool | a page load or continuation is in flight |
| `error` | string \| null | user-visible error state, if shown |
| `rail_selected` | string \| null | the selected rail/guide entry, as a page id (the same values as `page`: `home`, `subscriptions`, `history`, `watch_later`), never a display label; `null` on search, watch and channel; while the rail isn't drawn (narrow window, fullscreen) either `null` or the page's own id |
| `menu` | string \| null | an open popup: `card`, `player`, `settings`, `suggestions`, `guide`, `shortcuts`, `stats`, or another short name |
| `focus` | string \| null | the focused element: `search`, or `null` |
| `video` | object \| null | `{"id","status","position","duration","paused","muted","first_frame"}`; `status` is `idle`, `loading`, `ready`, `ended` or `failed`; `first_frame` is true once a frame of this id has been shown |
| `fullscreen` | bool | |
| `theatre` | bool | |
| `back_depth` | number | pages behind the current one |
| `forward_depth` | number | pages Forward would restore |
| `targets` | array | visible interactive elements, at most 80: `{"kind","rect":[x,y,w,h],"label"}` |

`targets` carries a `kind` (examples: `card`, `row`, `related`, `chip`, `rail`, `search_field`,
`search_button`, `suggestion`, `back`, `menu_item`, `player`, `play`, `scrubber`, `settings`,
`theatre`, `fullscreen`, `comment_replies`, `tab`, `subscribe`, `watch_later`, `share`, `more`,
`guide`), a `rect` in logical window pixels, and a `label` with the title or item text when
there is one. Only elements actually on screen and not covered by a popup are listed.

## Rules

- **Cost:** off by default and nearly free when off. It must not change timing enough to hide
  bugs: don't build the JSON unless `NT_PROBE=1`.
- **Shape:** the same field names and meanings in all three apps, so one explorer and one set
  of properties work on every framework.
- **Truthfulness:** report what is on screen, not what was requested (for example,
  `video.first_frame` only after a frame was actually presented).

For live-media tests, an `NT_AUTOPILOT` scenario can use
`{"cmd":"open_video_id","id":"5C_HPTJg5ek"}` to open a specific video through the normal Watch
path. Verify the exact ID and first frame in the probe before sending input. The CC button
uses kind `captions` and label `CC`. The Settings button is `settings` with label `Settings`.
Visible settings rows use kind `menu_item` and the stable labels `Subtitles/CC`, `Audio track`,
and `Auto-translate`. Painted, visible hit rectangles determine whether targets are present.

Live playback probes expose `video.captions` as `null` or `[track_index, translation_code]`.
GPUI and Rune expose `video.caption_lift` from 0 (resting) to 1 (lifted). This is the
position input to the renderer. Measure the rendered caption box in screenshots.
Rune exposes its playing track as `video.audio_track` and `video.audio_label`.
