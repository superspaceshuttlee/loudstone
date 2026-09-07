# Assets

Everything the game loads at runtime rather than compiles in.

| Folder | What it holds |
|---|---|
| `data/` | The content tables: blocks, items, recipes. RON, validated at startup. |
| `models/` | Reference model exports. Not loaded by the game; kept for authoring. |

## data/

These three files are the **source of truth** for what the game contains. They
are validated on load and a bad table is a hard startup error with a file, a
line and an explanation, never a silent default.

They are also compiled into the binary with `include_str!`. A copy on disk wins
where it exists, so the game can be retuned without a rebuild, and a shipped
binary still runs with no assets folder at all.

Ids are frozen. Save files store raw block and item numbers and are read back
against these tables, so renumbering an existing entry silently rewrites every
save that mentions it. Append; never renumber.

## Textures and audio

There are none, deliberately. Every block, item and mob skin is generated in
code (`src/render/texture.rs`), and every sound is synthesised
(`src/audio.rs`). The atlas is built once at startup and is reproducible from
the seed, which is why there is a test asserting two builds are byte-identical.

The tradeoff is honest: hand-painted art would look better, and this cannot be
edited by anyone who does not write Rust. What it buys is a repository with no
binary blobs, no licensing question about where the art came from, and a texture
that can be derived from the block's own data.
