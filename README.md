# Odeon — server

[![testes](https://github.com/Serious-Sam97/odeon-server/actions/workflows/testes.yml/badge.svg)](https://github.com/Serious-Sam97/odeon-server/actions/workflows/testes.yml)

> 🇧🇷 **Versão em português: [README.pt-BR.md](README.pt-BR.md)** — and that one
> is the original. The project is written in Portuguese: code comments, function
> names, API routes and the 7,900-line design document. This README is the front
> door, translated.

A media server of your own. Not a Jellyfin clone — an answer to what Jellyfin
does badly.

The thesis: **it isn't a file catalogue, it's a library that knows you.**

> **Read `docs/DESIGN.md`.** It's 7,900 lines and it records the **why** behind
> every choice in this project: what was measured, what was decided, and the
> defects found along the way. It's the part of this repository you can't get by
> reading the code. It is in Portuguese.

---

## Two repositories

Odeon is split in two. They talk over HTTP and nothing else — no shared types,
no generated code, no cross imports.

| | | |
|---|---|---|
| **[odeon-server](https://github.com/Serious-Sam97/odeon-server)** | **you are here** | Rust · axum + sqlx + Postgres/pgvector · the whole `docs/DESIGN.md` |
| **[odeon-client](https://github.com/Serious-Sam97/odeon-client)** | the interface | React + TypeScript, plus the Kotlin Multiplatform clients for phone, TV and iOS |

**This repository is a complete server.** It boots, walks your disk, identifies
your files, transcodes and serves `/api/…`. Anything that speaks HTTP consumes
it — the web interface is just the first such thing.

The client's `src/api.ts` describes these responses in hand-written TypeScript.
It was already a copy before the repositories split, and it stays a copy: a
route that changes shape here has nothing that warns the other side. That is the
debt the separation buys, and it was bought knowingly.

---

## Running it

Prerequisites: Docker. That's all — Rust, Node and FFmpeg run inside containers.

```bash
cp .env.example .env
```

Point `MEDIA_PATH` in `.env` at where your media lives **on the host**. It is
mounted at `/media` inside the container, **writable**.

> **The mount is writable because "delete from disk" in the manage drawer (R10)
> needs it.** If you don't want the server able to touch your files, put `:ro`
> back on the three `MEDIA_PATH` lines in `docker-compose.yml` and run
> `docker compose up -d`. Nothing breaks: the delete button goes back to being
> born disabled and explaining why, because `GET /api/storage` tests writing for
> real instead of inferring it from configuration.

```bash
docker compose up -d --build
```

The first boot compiles the backend from scratch inside the container (a few
minutes). After that `cargo watch` rebuilds only what changed.

| service | where |
|---|---|
| api | http://localhost:8080 |
| postgres | `localhost:5433` (`odeon` / `odeon`) |

**There is no web service here.** Bring up
[odeon-client](https://github.com/Serious-Sam97/odeon-client) — or point any
client at this API. Then create the administrator, open the **pastas** (folders)
tab to choose where your media is, and **scan**.

### Where the movies live

`MEDIA_PATH` in `.env` is the **root folder** the server can see. Inside it you
create as many libraries as you like through the interface — Movies, Shows,
Anime — each with its own kind and its own identifier (TMDB, AniList, or none).

The container only sees what is mounted into it, so libraries can only point
inside `MEDIA_PATH`. Nested libraries are refused: a file belongs to exactly one
library, so the inner one would be born empty.

**Disk on another machine?** Mount the share on the host first (on macOS, Finder
→ Go → Connect to Server → `smb://IP/folder`) and point `MEDIA_PATH` at
`/Volumes/NAME`. It works, but video travels machine → host → you. Running Odeon
on the machine that holds the disk is always better.

```bash
docker compose logs -f api              # follow the backend
docker compose exec db psql -U odeon    # open the database
```

---

## What already works

**M0 — backbone**

- Recursive disk walk with `ffprobe` on every file
- Graph data model — no `movie` or `tv_show` table
- **Direct Play** over HTTP Range: seeking works, nothing is transcoded
- `play_event` as a raw log + `playback_state` as the "continue watching" cache
- Full-text + trigram search

**M1 — identity**

- A parser that uses **directory context** (`Severance/Season 2/S02E07.mkv`
  finds its title two folders up) and understands **anime** (fansub group,
  absolute episode, `[SubsPlease] Frieren - 12 [1080p]`)
- **TMDB** (movies and shows) + **AniList** (anime, no API key needed)
- **Auditable confidence score**: every attempt becomes a row in
  `match_candidate` with the human-readable reasons behind the score
- **Review queue**: below 85% confidence the matcher **writes nothing** to the
  work — it flags and asks. Above 85% it goes in by itself.
- Artwork downloaded and served locally, with the **dominant colour** extracted
  from the poster
- Episodes create `collection(series)` → `collection(season)` automatically

**M2 — the graph**

- **Namespaced tags** (`mood:melancholic`, `genre:drama`) — TMDB/AniList genres
  arrive by themselves during matching
- **Recursive collections** with CRUD: franchise → series → season, as many
  levels as there are. The matcher creates series and seasons; playlists and
  orderings are yours.
- **Alternative watch orders** (`watch_order`), reorderable — "Machete order" is
  a first-class use case, not a workaround
- **Work↔work relations**: `alternate_cut_of`, `sequel_of`, `remake_of`… read
  from both sides out of a single edge
- **Compound filtering** in one query: tags (`all`/`any`), year range, runtime
  range, collection (the whole subtree), identification state, ordering

**M3 — the soul**

- **Seek preview**: a sprite sheet generated with ffmpeg, one image per file.
  Dragging the timeline shows the frame at that instant with no request at all —
  the browser already holds the whole sheet.
- **Custom player** (no `<video controls>`): timeline with buffer, keyboard
  shortcuts (space, ←/→, f, m), auto-hide, fullscreen
- **Dominant poster colour** extracted at match time. Since the redesign it
  belongs to the *art* — hero and player halo, poster border — and amber answers
  for the *system*: action, focus, timeline, score (see R1/R2)
- **Live sync across devices** over SSE: pausing on the laptop moves the TV.
  Each device ignores its own echo via `device_id`.
- A design system with a spacing scale, motion tokens and
  `prefers-reduced-motion`

**M5 — curation**

- **Taste profile derived from `play_event`** — nothing is declared. The strong
  signal is *finishing*, not *watching*: dropping out at eight minutes counts as
  rejection, and rewatching is the strongest positive there is. Decays by
  recency (60-day half-life).
- **Local embeddings** (TF-IDF + hashing trick, 256 dims) in **pgvector** — no
  external API, your library is never sent to a third party
- **Context**: "I have 40 minutes" is a hard filter; mood is `mood:` from M2
- **Every item says why** — "you usually finish talks (100%)", "but you often
  drop meetings", "you stopped with 72 min left"
- **An inspectable profile** right on screen: a recommendation that won't let
  itself be audited is guesswork
- Explicit feedback (♥ / ✕) for what behaviour can't infer

**M6 — heavy playback**

- **Auditable negotiation**: the player shows a badge (Direct Play / Remux /
  Transcode) and, on click, the reasons — *"the client can't play ac3 audio"*.
  It is the question Jellyfin never answers properly.
- **Real browser capability**, asked of the browser with `canPlayType` instead of
  a hardcoded list. Safari plays HEVC and gets Direct Play; the rest don't.
- **Acceleration detected by a real test encode** — `ffmpeg -encoders` lists what
  was compiled in, not what works. Every encoder is tested at boot, and a refusal
  comes with a reason.
- **On-demand HLS transcode** with sessions: 4s segments, aligned keyframes, and
  a reaper that kills idle sessions and frees the disk
- **Subtitles**: text becomes a native WebVTT track (no transcode); ASS/SSA and
  bitmaps (PGS/DVD) can be **burned in**, preserving the original styling

### HTTPS

Off by default. On a tailnet the transport is already encrypted, so the gain
isn't confidentiality — it's **secure context** in the browser: Service Workers,
offline PWA, `crypto.subtle` and part of the Media Session API only exist under
HTTPS.

**Don't use a self-signed certificate.** Every device would have to trust a new
CA, and installing a CA on Android TV is misery. Tailscale issues a real Let's
Encrypt certificate:

```bash
tailscale cert --cert-file certs/cert.crt --key-file certs/cert.key odeon.YOUR-TAILNET.ts.net
```

Then, in `.env`:

```
ODEON_TLS_CERT=/certs/cert.crt
ODEON_TLS_KEY=/certs/cert.key
ODEON_HTTPS_ONLY=true
```

`ODEON_HTTPS_ONLY=true` makes port 8080 redirect only (308, which preserves
POST). Leave it `false` while migrating.

**The clients sort themselves out.** In the apps you just type the host (`rog`):
they try https before http. On the web, the API is derived from the page itself
— same host, same scheme, port 8443 under HTTPS and 8080 under HTTP. There is no
hardcoded URL anywhere to update.

One detail that bites: **an HTTPS page cannot call an HTTP API** — the browser
blocks it as mixed content, and that includes `<video>`. The web interface
detects that combination and explains it, instead of looking like the server
went down.

To exercise the TLS layer without a tailnet: `./certs/dev-cert.sh` generates a
self-signed certificate — every browser will complain, and the TV won't accept
it.

### TMDB needs a key

Free, but it needs an account: https://www.themoviedb.org/settings/api
Put it in `TMDB_API_KEY` in `.env` and run `docker compose up -d api`.
**Without it Odeon still identifies anime** through AniList, which needs no key.

**Cast and crew**

- **TMDB** (directing, writing, cast, score) and **AniList** (staff and **voice
  actors with their character**) — in anime, who dubs is first-class information
- **Deduplication by `provider_key`**: "Villeneuve" is one person, not one row
  per film. Without it, "everything by Villeneuve" would return a single movie.
- **Library filtering by person** — clicking a name in the detail view filters
  everything
- **Per-person affinity in curation**: "you finished 2 works with Shinichirou
  Watanabe" becomes a recommendation reason. It requires 2+ works — with only
  one, the entire cast of a film you liked would become a "favourite taste".
- Portraits cached locally, same as artwork

**Authentication**

- **Multi-user** with **Argon2id** password hashing, `admin` and `user` roles
- **Revocable opaque sessions**, not JWT — logging out a lost device is deleting
  one row. The token never touches disk: its SHA-256 is what's stored.
- **First run is guarded**: while nobody has a password, `/api/auth/setup`
  answers; after that it is 403 forever
- Server operations (scan, identify, sprites, embeddings, libraries) are
  admin-only; watching and curating are for any user
- Changing your password drops your other sessions
- **CORS by the same-host rule**: a front end at `http://rog:5174` talking to the
  API at `http://rog:8080` passes on its own; `http://evil.com` doesn't. No
  configuration, and nothing breaks when the machine's name changes.

The first time you open the interface, it asks you to create the administrator.

---

## Roadmap

| | | |
|---|---|---|
| **M0** | Backbone | ✅ watch a movie from your own server |
| **M1** | Identity | ✅ TMDB + AniList, auditable score, review queue, artwork |
| **M2** | The graph | ✅ tags, recursive collections, relations, watch orders, filters |
| **M3** | The soul | ✅ seek preview, custom player, dominant colour, SSE sync |
| **M4** | Clients | ✅ Compose Multiplatform: phone, TV and iOS — all three build |
| **M5** | Curation | ✅ taste profile, pgvector, time/mood context, reasons |
| **M6** | Heavy playback | ✅ auditable negotiation, on-demand HLS, real hwaccel, ASS subtitles |
| **R1–R8** | Redesign | ✅ dashboard, player, library, collections, review queue, the work sheet |
| **R6/R13** | Live channels | ✅ IPTV over M3U + XMLTV, guide grid, and Odeon programming its own channels from your library |
| **R8/R11** | The video store | ✅ 600 VHS and DVD cases on genre shelves, in CSS 3D, that fly off the shelf and spin under the cursor |
| **R16** | Administration | ✅ people, devices, jobs and the four maintenance tasks, each rehearsed before it runs |
| **R18** | Cinema guide | ✅ directing, cast, score, genre and decade — each name with what you own and what you did with it |
| **R19/R29** | Lending, and scarcity | ✅ one copy per case; whoever took it took it off the shelf, and "ask for it back" is the way out |
| **R38–R46** | Round two | ✅ saga covers, an address for every screen, a Steam-like profile, watch-together |
| **R47–R51** | Round three | ✅ the face in the header, draggable rows, a folder picker that says what a folder *is*, borrowing required to press play, and this split |

Everything from R38 onward is recorded section by section in `docs/DESIGN.md`
(§54 to §67), with what was measured and what broke.

Transcoding stayed in M6 **on purpose**: it is the biggest complexity sink in the
project, and over Tailscale, on your own devices, Direct Play covers nearly
everything. Postponing it is what let the other five exist. See
[docs/DESIGN.md](docs/DESIGN.md) for the full reasoning.

---

## Layout

```
backend/          Rust — axum + tokio + sqlx
  migrations/     schema (the heart of the project lives here)
  src/scanner/    walk + ffprobe + filename parser
  src/metadata/   TMDB + AniList + confidence score
  src/routes/     HTTP API
docs/DESIGN.md    architecture decisions, and the why
```

---

## Licence

**AGPL-3.0.** You may use, modify and redistribute it — and if you serve a
modified version over a network, you publish that version's source.

It is the licence that matches what this software is: a server, used **through**
a network rather than distributed on disk. Under the plain GPL that obligation
would never fire, because serving isn't distributing.

See `LICENSE`.
