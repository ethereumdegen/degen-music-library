# Degen Music Library

[![Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Ratatui](https://img.shields.io/badge/TUI-Ratatui-7aa2f7)](https://ratatui.rs/)
[![License: MIT](https://img.shields.io/badge/license-MIT-9ece6a.svg)](#license)

A fast, keyboard-driven terminal music player for Cloudflare R2 and other S3-compatible object stores.

Degen Music Library turns an object-storage bucket into a browsable music library. Connect from the terminal, move through folders, select a track, and listen through your system's default audio output. No local media database, web server, mount, or background daemon required.

```text
┌ Library ───────────────────────────────────────────────────────────────┐
│ s3://music/Albums/                                                    │
└───────────────────────────────────────────────────────────────────────┘
┌ Bucket Browser ───────────────────────────────────────────────────────┐
│    [DIR]  Ambient/                                                    │
│ -> [AUDIO] Selected Track.flac                              38.4 MiB   │
│    [AUDIO] Another Track.mp3                                11.7 MiB   │
└───────────────────────────────────────────────────────────────────────┘
┌ Now Playing ──────────────────────────────────────────────────────────┐
│ Selected Track.flac  2:14 / 5:46                         [ === ]  80%  │
└───────────────────────────────────────────────────────────────────────┘
```

## Features

- Browse an S3 bucket as a folder-based music library.
- First-class Cloudflare R2 support.
- Works with path-style S3-compatible services such as MinIO.
- Play audio through the system's default output device.
- Automatic playback of the next track in the current folder.
- Pause, resume, stop, next, previous, and volume controls.
- Repeat the current track once or continuously without downloading it again.
- Toggle between the library and a real-time, 64-band FFT visualizer.
- Rapid `n`/`p` navigation uses cancellable, latest-request-wins loading.
- Track progress, duration, file size, and playback-state display.
- Paginated bucket listings for libraries with more than 1,000 objects.
- Case-insensitive audio extension detection.
- Keyboard-only workflow with Vim navigation keys.
- No credential files, local media index, analytics, or background service.

Supported extensions:

```text
aac  aif  aiff  alac  flac  m4a  mka  mp2  mp3
ogg  oga  opus  wav   wave  webm
```

Actual decoding support depends on the codec data inside the file and Rodio/Symphonia's decoder support.

## Install

### Install directly from GitHub

```bash
cargo install --git https://github.com/ethereumdegen/degen-music-library
```

Then run:

```bash
degen-music-library
```

### Build from source

```bash
git clone https://github.com/ethereumdegen/degen-music-library.git
cd degen-music-library
cargo run --release
```

The first build can take a few minutes because it compiles the AWS SDK and audio stack.

### Requirements

- A current Rust toolchain
- A working system audio output
- An S3-compatible bucket with permission to list and read objects

Install Rust with [rustup](https://rustup.rs/) if `cargo` is not already available.

## Cloudflare R2 setup

Create an R2 API token with the narrowest permissions the player needs: object read access for the music bucket. The application only calls `ListObjectsV2` and `GetObject`.

Enter these values on the connection screen:

| Field | Cloudflare R2 value |
|---|---|
| S3 endpoint | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` |
| Region | `auto` |
| Bucket | Your R2 bucket name |
| Access key ID | Access key from the R2 API token |
| Secret access key | Secret key from the R2 API token |

The account ID is shown in the Cloudflare dashboard. It is not the bucket name.

Buckets created in a specific jurisdiction require the matching endpoint:

| Jurisdiction | S3 endpoint |
|---|---|
| Default | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` |
| European Union | `https://<ACCOUNT_ID>.eu.r2.cloudflarestorage.com` |
| United States | `https://<ACCOUNT_ID>.us.r2.cloudflarestorage.com` |
| FedRAMP | `https://<ACCOUNT_ID>.fedramp.r2.cloudflarestorage.com` |

Do not use an `r2.dev` URL or a custom public domain as the S3 endpoint. Those URLs serve public objects; authenticated S3 API requests use the account endpoint above.

Cloudflare references:

- [Create R2 API tokens](https://developers.cloudflare.com/r2/api/tokens/)
- [R2 S3 API compatibility](https://developers.cloudflare.com/r2/api/s3/api/)

## Other S3-compatible services

Use the service's S3 API endpoint, signing region, bucket name, access key, and secret key. Endpoints without a scheme default to HTTPS.

Example for a local MinIO server:

```text
S3 endpoint:       http://127.0.0.1:9000
Region:            us-east-1
Bucket:            music
Access key ID:     <your access key>
Secret access key: <your secret key>
```

Degen Music Library uses path-style bucket addressing for compatibility with custom endpoints.

## Controls

### Connection screen

| Key | Action |
|---|---|
| `Tab` / `Down` | Next field |
| `Shift+Tab` / `Up` | Previous field |
| `Ctrl+U` | Clear the selected field |
| `Enter` | Connect and open the bucket |
| `Esc` | Quit |

### Library browser

| Key | Action |
|---|---|
| `Up` / `k` | Move selection up |
| `Down` / `j` | Move selection down |
| `Page Up` / `Page Down` | Move by ten items |
| `Home` / `End` | Jump to the first or last item |
| `Enter` / `Right` / `l` | Open a folder or play a track |
| `Backspace` / `Left` / `h` | Go to the parent folder |
| `Space` | Pause or resume |
| `n` / `p` | Next or previous track |
| `o` | Cycle loop mode: Off → Once → Forever |
| `v` | Toggle between the library and FFT visualizer |
| `+` / `-` | Increase or decrease volume |
| `x` | Stop playback |
| `r` | Reload the current folder |
| `c` | Disconnect and enter new credentials |
| `q` / `Esc` | Quit |

`Ctrl+C` exits from either screen.

**Loop Once** replays the current track one additional time, then returns to normal playback and advances. **Loop Forever** replays the current track until you change the mode, select another track, or stop playback.

Rapid navigation never queues tracks. Each `n` or `p` press stops current playback, moves from the latest pending selection, aborts the obsolete request, and starts only the final requested track. The app does not speculatively prefetch neighboring objects.

## Security and privacy

- Credentials are entered interactively and are never written to disk.
- The secret access key is masked in the terminal.
- Access-key and secret-key form buffers are zeroed after a successful connection.
- Credentials remain in the in-memory AWS SDK client only for the active session.
- Audio objects are downloaded to managed temporary files rather than retained in the music directory.
- Temporary audio files are removed when the cached track is dropped or the application exits.
- The application contains no telemetry or analytics integration.

For Cloudflare R2, prefer a bucket-scoped, read-only API token. Do not reuse an account-wide administrative token.

## How playback works

1. The app requests the current prefix with S3 `ListObjectsV2` and `/` as the delimiter.
2. Common prefixes become folders; supported audio objects become tracks.
3. Selecting a track downloads it asynchronously into a managed temporary file.
4. Rodio and Symphonia detect and decode the audio format.
5. A lightweight sample tap feeds a Hann-windowed, 2,048-point FFT for the visualizer.
6. Playback begins through the default output device.
7. At track end, the selected loop mode reopens the cached file or normal playback advances.

The complete object is downloaded before playback begins. This keeps memory usage bounded and decoding reliable, but very large tracks or slow connections can take a moment to start.

## Troubleshooting

### `Connection failed`

Check all five connection fields. For R2, the region should be `auto`, the endpoint should contain the Cloudflare account ID, and the token must have access to the selected bucket.

### The bucket connects but appears empty

Only folders and supported audio extensions are displayed. Confirm that the objects have one of the extensions listed above and that the API token can list the bucket.

### A track downloads but does not play

The extension may be supported while the file's codec or container contents are not. Confirm the file plays locally and is not encrypted or corrupted.

### No audio output device

Confirm that the operating system has a working default output device before starting the application.

## Development

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo run
```

Project layout:

```text
src/
├── app.rs      application state, input handling, and async task coordination
├── player.rs   audio device, decoder, and playback controls
├── storage.rs  S3 connection, listing, filtering, and downloads
├── visualizer.rs audio sample capture and FFT spectrum analysis
├── ui.rs       Ratatui rendering
└── main.rs     executable entry point
```

## License

MIT
