# filmcraft-mxf

A clean-room MXF demuxer. Layer L0: no dependencies beyond `std`, no `unsafe`, builds for
`wasm32-unknown-unknown`. No GPL/LGPL code (FFmpeg, libMXF, bmx, MXFLib) was consulted; FFmpeg is
used only as an external fixture generator and test oracle.

## Specifications

Implemented from the SMPTE documents (editions used):

| Document | Edition | Used for |
|---|---|---|
| SMPTE ST 336 | 2017 | KLV coding: 16-byte UL keys, BER lengths |
| SMPTE ST 377-1 | 2011 (+ Amd 1:2012) | partitions, primer pack, header metadata sets and local tags, index table segments, random index pack, run-in |
| SMPTE ST 378 | 2004 | OP1a |
| SMPTE ST 390 | 2011 | OP-Atom |
| SMPTE ST 379-1 / 379-2 | 2009 / 2010 | generic container: content packages, element keys, frame / clip wrapping |
| SMPTE ST 381-1 | 2005 | MPEG video mapping (MPEG-2 identified, MPEG video descriptor) |
| SMPTE ST 381-3 | 2013 | AVC byte-stream mapping and AVC sub-descriptor |
| SMPTE ST 382 | 2007 | AES3 and Broadcast Wave audio mapping (wave / AES3 descriptors) |
| SMPTE ST 331 | 2011 | element data of 8-channel AES3 sound (D-10) |
| SMPTE ST 386 | 2004 | D-10 (IMX) mapping: container label, sound element type |
| SMPTE ST 2019-4 | 2009 | VC-3 (DNxHD / DNxHR) mapping |
| SMPTE RDD 44 | 2017 | Apple ProRes mapping |
| SMPTE RP 224 / RP 210 | registers | labels: data definitions, picture coding, essence containers, operational patterns |

## Reading a file

```rust
let file = std::fs::read("clip.mxf")?;
let mxf = filmcraft_mxf::open(&file)?;
println!("{} {:?}", mxf.operational_pattern.name(), mxf.timecode.map(|t| t.format()));
let v = mxf.track_of_kind(filmcraft_mxf::TrackKind::Picture).unwrap();
let t = &mxf.tracks[v];
let key = t.sync_before(t.sample_at(42).unwrap());    // stored index of the random-access picture
let bytes = mxf.read_sample(&file, v, key)?;           // one edit unit's essence
let a = mxf.track_of_kind(filmcraft_mxf::TrackKind::Sound).unwrap();
let pcm = mxf.read_pcm(&file, a, 48_000, 1920)?;       // planar f32, sample-exact
```

- **Open.** The header partition pack is found in the run-in (≤ 64 KiB). Every KLV packet is then
  walked (keys and lengths only; essence values are skipped, never read): partition packs, primer
  packs, header metadata local sets, index table segments, fill, and generic-container elements
  (system items are ignored). A lost KLV sync resynchronises on the next partition pack; a
  truncated file keeps everything before the cut (a truncated frame-wrapped picture is dropped,
  clip-wrapped essence keeps its whole edit units).
- **Metadata.** The header metadata of the most complete partition is used (closed complete >
  open complete > closed > open; later partitions win ties, so the footer's final durations are
  preferred). Dynamic local tags resolve through the primer pack (e.g. `SubDescriptors`).
- **Tracks.** Material package picture/sound tracks → source clip → file package track (track
  number, edit rate, origin) → descriptor (a multiple descriptor's sub-descriptor by
  `LinkedTrackID`). Without a usable material package the file packages' tracks are used directly.
  Elements are matched by track number, or — when the number is 0 or does not match (some OP-Atom
  writers) — the only unclaimed element stream of the right kind.
- **Pictures.** One `Sample` per edit unit in stored order: frame wrapping (one element each) or
  clip wrapping (one element split by the index: constant `EditUnitByteCount` or VBR entries).
  Random access from index flag bit 7 (intra-only codings: every picture); without an index the
  essence is scanned (AVC IDR, MPEG-2 sequence header). Presentation order from the index temporal
  offsets (display position *n* is stored at *n* + `TemporalOffset[n]`). When the index marks B
  pictures but has no temporal offsets (FFmpeg-written AVC), `needs_reorder` is set: the caller
  orders by the bitstream (`filmcraft-codecs` uses the AVC picture order counts).
- **Codecs.** From the picture essence coding label, then the essence container label, then the
  essence bytes: AVC / AVC-Intra, VC-3, ProRes (profile from the label), MPEG-2 (identified only),
  MPEG-4 visual, JPEG 2000, DV, uncompressed.
- **Sound.** PCM chunks (frame- or clip-wrapped) with sample counts; `read_pcm` decodes
  little-endian 8/16/24/32-bit PCM (wave / AES3 descriptors) and ST 331 AES3 elements (D-10).
- **Timecode.** The material package timecode component (start frame count, rounded base,
  drop frame), falling back to the file package's; `Timecode::format` gives `HH:MM:SS:FF` /
  `HH:MM:SS;FF`.

Not supported: OP1b/OP2x+ edit lists beyond the first source clip, external essence (OP-Atom
material packages whose other tracks live in other files are opened track by track), partial or
encrypted essence (ST 429-6), and descriptive metadata (parsed sets are ignored).

## Tests

- `src/tests.rs`: a small OP1a writer builds synthetic files (temporal offsets, key flags, PCM,
  timecode); open, presentation order, sample bytes, PCM values; truncation at every 37 bytes and
  600 random mutations never panic and only list fully-present samples.
- `crates/codecs/tests/mxf_oracle.rs` (FFmpeg-written fixtures, FFmpeg as the decode oracle):
  H.264 long-GOP with B pictures (bit-exact every frame + 25 random seeks), H.264 29.97 DF
  timecode, DNxHR LB (±2), ProRes 422 (±1), MPEG-2 (reported unsupported, audio exact),
  OP-Atom VC-3 (clip-wrapped) and PCM, D-10 AES3 audio; PCM sample-exact in every file;
  truncated and corrupted files. `cargo xtask fixtures codecs` pre-generates them.
