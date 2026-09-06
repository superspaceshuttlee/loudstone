//! Audible sound -- the noise the *player* hears.
//!
//! This module is not `sound.rs`. `sound.rs` is the simulation: it floods
//! loudness through the voxel grid so mobs can decide what they heard. This one
//! pushes samples at the speakers. They never talk to each other.
//!
//! # Everything is synthesised
//!
//! Loudstone ships zero asset files, and that includes audio. Every sound here
//! is built from filtered noise, damped oscillators and envelopes, rendered once
//! into a bank of small buffers at startup. A stone chip is a short burst of
//! band-passed noise around 1.5 kHz; wood is lower and has a triangle body under
//! it, so it thunks; diamond and ice ring, because their body decays slowly at a
//! high frequency. Change the numbers in the tuning block and the material
//! changes character.
//!
//! # Nothing here can stall or crash the game
//!
//! * `Audio::new()` returns immediately. Device enumeration, bank rendering and
//!   stream creation all happen on a dedicated thread.
//! * `play()` computes a gain, checks a cooldown, and drops a 16-byte message
//!   into a bounded queue with `try_send`. It never allocates, never blocks, and
//!   silently drops the sound if the queue is full or the lock is contended.
//! * If there is no audio device, the whole thing degrades to a stub: one line
//!   on stderr at startup and `play()` becomes a no-op. The game is unaffected.
//! * The audio thread runs inside `catch_unwind`, so even a misbehaving driver
//!   backend costs you sound, not the process.
//!
//! Set `LOUDSTONE_NO_AUDIO=1` in the environment to force the silent stub.

// This module is shaped like a small library: it exposes a complete API (the
// silent stub, the master-volume readback, the `PlayOpts` builders, the mixer
// introspection used by the tests) of which the game calls a subset. Without
// this the binary's dead-code pass reports every unused entry point.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Instant;

use glam::Vec3;

use crate::block::BlockId;

// =============================================================================
// ============================  TUNING BLOCK  =================================
//   Everything dialable about what the game sounds like lives here. Nothing
//   below this block is a magic number. Turn these knobs, not the code.
// =============================================================================

// ----------------------------- mix and output --------------------------------

/// Where the master fader starts. `set_master_volume` overrides it at runtime.
pub const DEFAULT_MASTER_VOLUME: f32 = 0.70;

/// Hard cap on simultaneous voices. Beyond this the quietest playing voice is
/// stolen. Higher costs CPU in the audio callback and muddies dense moments.
pub const MAX_VOICES: usize = 24;

/// Where the output limiter starts bending. Below this the mix is untouched;
/// above it, sums are squashed smoothly toward the ceiling and can never clip.
/// Lower is safer and squashier, higher is more dynamic and closer to the edge.
pub const LIMITER_KNEE: f32 = 0.70;

/// The loudest sample the mixer can ever emit. Held just under full scale so
/// the output is provably inside [-1, 1] with a sliver to spare.
pub const LIMITER_CEILING: f32 = 0.995;

/// Depth of the game-thread -> audio-thread queue, in messages. A frame that
/// tries to fire more than this many sounds drops the overflow.
pub const COMMAND_QUEUE: usize = 128;

// --------------------------- distance attenuation ----------------------------

/// Distance at which a positioned sound is at half power, in blocks.
/// Attenuation is `1 / (1 + (d / this)^2)`, the same shape `sound.rs` uses.
pub const ATTENUATION_HALF_DISTANCE: f32 = 7.0;

/// Beyond this many blocks a positioned sound is not queued at all.
pub const MAX_AUDIBLE_DISTANCE: f32 = 48.0;

/// Final gains below this are dropped rather than queued. Also the NaN trap.
pub const MIN_AUDIBLE_GAIN: f32 = 0.002;

// ------------------------------ per-sound gain -------------------------------
//   Relative loudness of each sound before distance and master. Every rendered
//   buffer is normalised to the same peak, so these numbers are the *only*
//   place loudness balance lives.

pub const GAIN_DIG: f32 = 0.30;
pub const GAIN_BREAK: f32 = 0.70;
pub const GAIN_PLACE: f32 = 0.55;
pub const GAIN_FOOTSTEP: f32 = 0.26;
pub const GAIN_PLAYER_HURT: f32 = 0.85;
pub const GAIN_MOB_HURT: f32 = 0.60;
pub const GAIN_MOB_DEATH: f32 = 0.70;
pub const GAIN_FUSE: f32 = 0.55;
pub const GAIN_EXPLOSION: f32 = 1.00;
pub const GAIN_BOW: f32 = 0.50;
pub const GAIN_ARROW_HIT: f32 = 0.45;
pub const GAIN_DRIP: f32 = 0.35;
pub const GAIN_WIND: f32 = 0.20;

// -------------------------------- cooldowns ----------------------------------
//   Minimum seconds between two plays of the same *kind* of sound, regardless
//   of material or position. This is what stops the mining tick turning into a
//   machine-gun buzz: chipping dirt retriggers every 36 ms, and without a floor
//   here that is 28 identical bursts a second.

pub const COOLDOWN_DIG: f32 = 0.085;
pub const COOLDOWN_BREAK: f32 = 0.050;
pub const COOLDOWN_PLACE: f32 = 0.080;
pub const COOLDOWN_FOOTSTEP: f32 = 0.170;
pub const COOLDOWN_PLAYER_HURT: f32 = 0.220;
pub const COOLDOWN_MOB_HURT: f32 = 0.090;
pub const COOLDOWN_MOB_DEATH: f32 = 0.090;
/// The fuse is re-triggered by the game loop every frame while a creeper is
/// primed; this is what turns that into one continuous hiss. Keep it a little
/// under `FUSE_LEN` so the bursts overlap instead of gapping.
pub const COOLDOWN_FUSE: f32 = 0.400;
pub const COOLDOWN_EXPLOSION: f32 = 0.100;
pub const COOLDOWN_BOW: f32 = 0.080;
pub const COOLDOWN_ARROW_HIT: f32 = 0.060;
pub const COOLDOWN_AMBIENCE: f32 = 0.500;

// ------------------------------- pitch jitter --------------------------------
//   Fractional +/- range of random playback-rate variation per trigger. This is
//   the single cheapest thing that stops repeated sounds sounding canned; the
//   dig and footstep numbers matter most because those repeat the most.

pub const JITTER_DIG: f32 = 0.17;
pub const JITTER_FOOTSTEP: f32 = 0.15;
pub const JITTER_BREAK: f32 = 0.10;
pub const JITTER_HURT: f32 = 0.12;
pub const JITTER_EXPLOSION: f32 = 0.10;
pub const JITTER_DEFAULT: f32 = 0.07;

/// Absolute clamp on the final playback rate, so a silly `PlayOpts::pitch` can
/// never turn a 0.1 s buffer into a 10 s drone or a supersonic click.
pub const PITCH_MIN: f32 = 0.25;
pub const PITCH_MAX: f32 = 4.00;

// ------------------------------- synthesis -----------------------------------

/// Peak every rendered buffer is normalised to. Leaves a hair of headroom under
/// 1.0 so a single voice at gain 1.0 cannot reach full scale on its own.
pub const RENDER_PEAK: f32 = 0.98;

/// Shortest attack ramp, in seconds. Anything shorter starts with a click, which
/// on a synthesised transient reads as a broken speaker rather than a sharp hit.
pub const ATTACK_MIN: f32 = 0.0012;

/// Every buffer is faded to exactly zero over this many seconds at its end, so
/// a voice that runs out never leaves a step in the mix.
pub const TAIL_FADE: f32 = 0.006;

/// How many differently-seeded renders of each sound the bank holds. One is
/// enough with pitch jitter alone; three is where repeated digging stops having
/// an audible "shape" you recognise.
pub const VARIANTS: usize = 3;

/// Fractional centre-frequency spread between those variants.
pub const VARIANT_DETUNE: f32 = 0.06;

// -------------------------- lengths of the one-shots -------------------------

pub const HURT_LEN: f32 = 0.30;
pub const DEATH_LEN: f32 = 0.55;
pub const FUSE_LEN: f32 = 0.55;
pub const EXPLOSION_LEN: f32 = 1.60;
pub const BOW_LEN: f32 = 0.26;
pub const ARROW_HIT_LEN: f32 = 0.13;
pub const DRIP_LEN: f32 = 0.30;
pub const WIND_LEN: f32 = 3.00;

// -------------------------------- footsteps ----------------------------------

/// Blocks walked per footstep, for the `footstep` helper.
pub const FOOTSTEP_STRIDE: f32 = 2.1;
/// Below this speed (blocks/second) the helper emits nothing.
pub const FOOTSTEP_MIN_SPEED: f32 = 0.6;
/// How far into the stride the accumulator is parked while standing still, so
/// the first step after you start walking lands promptly but not instantly.
pub const FOOTSTEP_PRIME: f32 = 0.75;

// --------------------------------- ambience ----------------------------------

/// Shortest and longest gap between ambient one-shots, in seconds.
pub const AMBIENCE_MIN_GAP: f32 = 14.0;
pub const AMBIENCE_MAX_GAP: f32 = 45.0;
/// How far from the listener an ambient sound is placed, in blocks.
pub const AMBIENCE_DISTANCE: f32 = 6.0;

// ------------------------- per-material voice tables -------------------------
//   The character of each material family. `centre`/`q` shape the noise burst,
//   `body` is the pitched thing under it. Raise `body_mix` for a more musical
//   hit, raise `q` for a more resonant one, set `grain` for a crunch.

/// One material's sonic fingerprint.
#[derive(Copy, Clone, Debug)]
pub struct MaterialVoice {
    /// Band-pass centre frequency of the noise burst, in Hz.
    pub centre: f32,
    /// Band-pass resonance. Low is a broad hiss, high is a narrow ring.
    pub q: f32,
    /// Multiplier applied to `centre` by the end of the burst. Below 1.0 the
    /// noise falls in pitch as it decays, which reads as "heavy".
    pub sweep: f32,
    /// Sample-and-hold rate on the noise, in Hz. 0 is smooth white noise;
    /// a few kHz gives the gritty rattle that gravel and snow need.
    pub grain: f32,
    /// Frequency of the pitched body under the noise, in Hz. 0 for none.
    pub body: f32,
    /// How much of that body to mix in, 0..1.
    pub body_mix: f32,
    /// Body decay time constant, in seconds. Long is a ring, short is a thud.
    pub body_tau: f32,
    /// Triangle body instead of sine. Triangles are woodier and buzzier.
    pub tri: bool,
    /// Per-material loudness trim, multiplied into the per-sound gain.
    pub trim: f32,
}

/// Rock: a bright chip with a dull thud under it.
pub const VOICE_STONE: MaterialVoice = MaterialVoice {
    centre: 1500.0, q: 1.2, sweep: 0.75, grain: 0.0,
    body: 210.0, body_mix: 0.28, body_tau: 0.035, tri: false, trim: 1.00,
};
/// Soil: low, soft, almost no transient.
pub const VOICE_DIRT: MaterialVoice = MaterialVoice {
    centre: 700.0, q: 0.8, sweep: 0.65, grain: 0.0,
    body: 120.0, body_mix: 0.30, body_tau: 0.030, tri: false, trim: 0.85,
};
/// Sand: pure broadband hiss, no pitch at all.
pub const VOICE_SAND: MaterialVoice = MaterialVoice {
    centre: 3300.0, q: 0.5, sweep: 0.55, grain: 0.0,
    body: 0.0, body_mix: 0.0, body_tau: 0.02, tri: false, trim: 0.80,
};
/// Gravel: sand with stones in it. The grain is what makes it rattle.
pub const VOICE_GRAVEL: MaterialVoice = MaterialVoice {
    centre: 2300.0, q: 1.0, sweep: 0.60, grain: 5200.0,
    body: 180.0, body_mix: 0.16, body_tau: 0.025, tri: false, trim: 0.90,
};
/// Snow: a high, dry squeak-crunch.
pub const VOICE_SNOW: MaterialVoice = MaterialVoice {
    centre: 4600.0, q: 0.7, sweep: 0.70, grain: 2600.0,
    body: 0.0, body_mix: 0.0, body_tau: 0.02, tri: false, trim: 0.70,
};
/// Wood: the woodiest thing here is the triangle body, not the noise.
pub const VOICE_WOOD: MaterialVoice = MaterialVoice {
    centre: 950.0, q: 1.7, sweep: 0.70, grain: 0.0,
    body: 320.0, body_mix: 0.55, body_tau: 0.055, tri: true, trim: 1.00,
};
/// Leaves: a long, quiet, high rustle with no body whatsoever.
pub const VOICE_LEAVES: MaterialVoice = MaterialVoice {
    centre: 5200.0, q: 0.45, sweep: 0.80, grain: 0.0,
    body: 0.0, body_mix: 0.0, body_tau: 0.02, tri: false, trim: 0.55,
};
/// Ore-bearing rock: stone, plus a metallic ring that outlasts the chip.
pub const VOICE_METAL: MaterialVoice = MaterialVoice {
    centre: 2100.0, q: 2.6, sweep: 0.85, grain: 0.0,
    body: 880.0, body_mix: 0.48, body_tau: 0.110, tri: false, trim: 1.00,
};
/// Diamond and ice: a glassy ping that rings on well past the transient.
pub const VOICE_CRYSTAL: MaterialVoice = MaterialVoice {
    centre: 3600.0, q: 3.4, sweep: 0.95, grain: 0.0,
    body: 1760.0, body_mix: 0.62, body_tau: 0.190, tri: false, trim: 0.95,
};
/// Water: a bloop. The body sweeps *up*, which is the whole trick.
pub const VOICE_LIQUID: MaterialVoice = MaterialVoice {
    centre: 1100.0, q: 0.9, sweep: 0.50, grain: 0.0,
    body: 380.0, body_mix: 0.55, body_tau: 0.070, tri: false, trim: 0.75,
};

// ------------------------- per-action envelope shapes ------------------------
//   How each *action* reshapes the material voice above. A break is the same
//   material heard longer and lower; a footstep is duller and shorter.

/// How one action bends a material's voice.
#[derive(Copy, Clone, Debug)]
pub struct ActionShape {
    /// Buffer length in seconds.
    pub len: f32,
    /// Attack ramp in seconds.
    pub attack: f32,
    /// Noise decay time constant in seconds.
    pub tau: f32,
    /// Multiplier on the material's centre frequency.
    pub centre_scale: f32,
    /// Multiplier on the material's body mix.
    pub body_scale: f32,
    /// Multiplier on the material's body decay.
    pub body_tau_scale: f32,
}

/// The repeating tick while mining. Short and dry above all else.
pub const SHAPE_DIG: ActionShape = ActionShape {
    len: 0.10, attack: 0.0015, tau: 0.022,
    centre_scale: 1.00, body_scale: 0.60, body_tau_scale: 0.55,
};
/// The pop when the block finally goes. Longer, lower, fuller.
pub const SHAPE_BREAK: ActionShape = ActionShape {
    len: 0.38, attack: 0.0015, tau: 0.075,
    centre_scale: 0.88, body_scale: 1.25, body_tau_scale: 1.60,
};
/// Setting a block down: a firm, short knock.
pub const SHAPE_PLACE: ActionShape = ActionShape {
    len: 0.18, attack: 0.0020, tau: 0.038,
    centre_scale: 1.05, body_scale: 1.00, body_tau_scale: 0.80,
};
/// A boot landing: duller and shorter than digging the same block.
pub const SHAPE_STEP: ActionShape = ActionShape {
    len: 0.15, attack: 0.0030, tau: 0.032,
    centre_scale: 0.72, body_scale: 0.85, body_tau_scale: 0.90,
};

// =============================================================================
// ==========================  END OF TUNING BLOCK  ============================
// =============================================================================

// Guard rails on the knobs above, checked at compile time so a knob dialled into
// nonsense is a build error rather than something you have to hear to notice.
const _: () = assert!(RENDER_PEAK > 0.0 && RENDER_PEAK < 1.0);
const _: () = assert!(LIMITER_KNEE > 0.0 && LIMITER_KNEE < LIMITER_CEILING);
const _: () = assert!(LIMITER_CEILING < 1.0);
const _: () = assert!(MAX_VOICES > 0);
const _: () = assert!(COMMAND_QUEUE > 0);
const _: () = assert!(VARIANTS > 0);
const _: () = assert!(PITCH_MIN > 0.0 && PITCH_MAX > PITCH_MIN);
const _: () = assert!(ATTENUATION_HALF_DISTANCE > 0.0);
const _: () = assert!(MAX_AUDIBLE_DISTANCE > ATTENUATION_HALF_DISTANCE);
const _: () = assert!(FOOTSTEP_STRIDE > 0.0);
const _: () = assert!(FOOTSTEP_PRIME >= 0.0 && FOOTSTEP_PRIME < 1.0);
const _: () = assert!(AMBIENCE_MAX_GAP > AMBIENCE_MIN_GAP);
// The mining tick retriggers as fast as every 36 ms on soft blocks. Anything
// under about 40 ms here and mining becomes a buzz rather than a rhythm.
const _: () = assert!(COOLDOWN_DIG > 0.04);
// The fuse is one short burst replayed on a cooldown; if the cooldown were
// longer than the burst, the hiss would stutter instead of sustaining.
const _: () = assert!(COOLDOWN_FUSE < FUSE_LEN);

// -----------------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------------

/// Everything the game can ask to hear. Material-keyed variants take the block
/// that was hit; the mapping to a sound family happens in here, so callers never
/// think about it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sound {
    /// The repeating tick while chipping a block.
    Dig(BlockId),
    /// The louder pop when a block finally goes.
    Break(BlockId),
    /// Setting a block down.
    Place(BlockId),
    /// One boot landing on the given ground block.
    Footstep(BlockId),
    PlayerHurt,
    MobHurt,
    MobDeath,
    /// A creeper priming. Re-trigger it every frame while the fuse burns; the
    /// cooldown turns that into one continuous hiss.
    CreeperFuse,
    Explosion,
    BowShot,
    ArrowHit,
    /// Ambience: a single cave drip.
    CaveDrip,
    /// Ambience: a low gust.
    Wind,
}

/// Per-trigger modifiers. `PlayOpts::default()` is "nominal volume, nominal
/// pitch, no position", which is right for anything that happens to the player
/// themselves.
#[derive(Copy, Clone, Debug)]
pub struct PlayOpts {
    /// Extra gain multiplier. 1.0 is the tuned default for that sound.
    pub volume: f32,
    /// Extra playback-rate multiplier, on top of the random jitter. 1.0 is
    /// nominal, 0.5 is an octave down, 2.0 an octave up.
    pub pitch: f32,
    /// Where the sound happens. `None` means "on the listener": no attenuation.
    pub pos: Option<Vec3>,
    /// Where the player's ears are. Ignored when `pos` is `None`.
    pub listener: Vec3,
}

impl Default for PlayOpts {
    fn default() -> Self {
        Self { volume: 1.0, pitch: 1.0, pos: None, listener: Vec3::ZERO }
    }
}

impl PlayOpts {
    /// Something that happened to the player. No distance attenuation.
    pub fn ui() -> Self {
        Self::default()
    }

    /// Something that happened out in the world, heard from `listener`.
    pub fn at(pos: Vec3, listener: Vec3) -> Self {
        Self { pos: Some(pos), listener, ..Self::default() }
    }

    /// Scale the volume. Chainable.
    pub fn with_volume(mut self, v: f32) -> Self {
        self.volume = v;
        self
    }

    /// Scale the pitch. Chainable.
    pub fn with_pitch(mut self, p: f32) -> Self {
        self.pitch = p;
        self
    }
}

/// The audio device, or a silent stub standing in for one.
///
/// Construct one at startup, keep it alive for the life of the game, and call
/// [`Audio::play`] from the game loop. Every method is safe to call at any time,
/// from any thread, whether or not a device exists.
pub struct Audio {
    /// `None` when there is no device: every `play` becomes a no-op.
    tx: Option<Mutex<SyncSender<Cmd>>>,
    /// Master gain, as `f32::to_bits`, read once per audio callback.
    master: Arc<AtomicU32>,
    /// Set on drop; the engine thread notices and lets the stream go.
    shutdown: Arc<AtomicBool>,
    /// Microseconds since `start` at which each sound kind last played.
    cooldowns: [AtomicU64; COOLDOWN_SLOTS],
    /// Lock-free xorshift, for variant choice and pitch jitter.
    rng: AtomicU32,
    /// Distance walked since the last footstep, as `f32::to_bits`.
    walked: AtomicU32,
    /// Seconds until the next ambient one-shot, as `f32::to_bits`.
    ambience: AtomicU32,
    start: Instant,
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio {
    /// Open the default output device and start the mixer.
    ///
    /// Never fails and never blocks: the device is opened on a background
    /// thread, so this returns in microseconds. If no device turns up, or the
    /// backend refuses, the result is a working silent stub and one line on
    /// stderr. Honours `LOUDSTONE_NO_AUDIO=1`.
    pub fn new() -> Self {
        if std::env::var_os("LOUDSTONE_NO_AUDIO").is_some() {
            eprintln!("[loudstone] audio: disabled by LOUDSTONE_NO_AUDIO");
            return Self::silent();
        }

        let (tx, rx) = sync_channel::<Cmd>(COMMAND_QUEUE);
        let master = Arc::new(AtomicU32::new(DEFAULT_MASTER_VOLUME.to_bits()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let spawned = std::thread::Builder::new()
            .name("loudstone-audio".to_string())
            .spawn({
                let master = Arc::clone(&master);
                let shutdown = Arc::clone(&shutdown);
                move || engine::run_guarded(rx, master, shutdown)
            });

        match spawned {
            Ok(_handle) => {
                // Deliberately not keeping the join handle: nothing in the game
                // ever waits for the audio thread, including at shutdown.
                let mut audio = Self::stub_parts(master, shutdown);
                audio.tx = Some(Mutex::new(tx));
                audio
            }
            Err(e) => {
                eprintln!("[loudstone] audio: could not start the audio thread ({e}) -- silent");
                Self::silent()
            }
        }
    }

    /// A stub with no device behind it. `play` does nothing, everything else
    /// behaves normally. Useful for headless runs and tests.
    pub fn silent() -> Self {
        Self::stub_parts(
            Arc::new(AtomicU32::new(DEFAULT_MASTER_VOLUME.to_bits())),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn stub_parts(master: Arc<AtomicU32>, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            tx: None,
            master,
            shutdown,
            cooldowns: std::array::from_fn(|_| AtomicU64::new(0)),
            rng: AtomicU32::new(0x5EED_1337),
            walked: AtomicU32::new((FOOTSTEP_STRIDE * FOOTSTEP_PRIME).to_bits()),
            ambience: AtomicU32::new(AMBIENCE_MIN_GAP.to_bits()),
            start: Instant::now(),
        }
    }

    /// Whether a real device is behind this. False means every `play` is a
    /// no-op; nothing else changes.
    pub fn is_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Fire and forget. Cheap enough to call from the middle of the game loop:
    /// a handful of multiplies, one atomic load, one non-blocking send. Sounds
    /// that are too quiet, too distant, or too soon after the last of their kind
    /// are dropped here and never reach the mixer.
    pub fn play(&self, sound: Sound, opts: PlayOpts) {
        let Some(tx) = self.tx.as_ref() else { return };

        // Distance first: it is the cheapest way to reject a sound.
        let mut gain = sound.gain() * opts.volume;
        if let Some(p) = opts.pos {
            let d2 = p.distance_squared(opts.listener);
            // The finite check is what rejects a NaN position.
            if !d2.is_finite() || d2 > MAX_AUDIBLE_DISTANCE * MAX_AUDIBLE_DISTANCE {
                return;
            }
            gain *= 1.0 / (1.0 + d2 / (ATTENUATION_HALF_DISTANCE * ATTENUATION_HALF_DISTANCE));
        }
        if !gain.is_finite() || gain <= MIN_AUDIBLE_GAIN {
            return;
        }

        // Then the cooldown, which is what keeps the mining tick from buzzing.
        let now = self.start.elapsed().as_micros() as u64;
        let slot = sound.cooldown_slot();
        let last = self.cooldowns[slot].load(Ordering::Relaxed);
        if !cooldown_ready(last, now, sound.cooldown()) {
            return;
        }
        // `now.max(1)` because 0 is the "never played" sentinel and the first
        // few microseconds of the process would otherwise look like it.
        self.cooldowns[slot].store(now.max(1), Ordering::Relaxed);

        let r = self.next_rand();
        let variant = (r >> 13) as usize % VARIANTS;
        let jitter = sound.jitter();
        // Map the low 16 bits to -1..1 and spread the rate around 1.0.
        let spread = (r & 0xFFFF) as f32 / 32_768.0 - 1.0;
        let rate = (opts.pitch * (1.0 + spread * jitter)).clamp(PITCH_MIN, PITCH_MAX);

        let cmd = Cmd {
            slot: (sound.slot_base() + variant) as u16,
            rate,
            gain: gain.min(4.0),
        };

        // try_lock, not lock: a contended send drops the sound rather than
        // parking the game loop. try_send, not send: a full queue does the same.
        if let Ok(guard) = tx.try_lock() {
            let _ = guard.try_send(cmd);
        }
    }

    /// Master fader, 0.0 to about 2.0. Applied on the audio thread before the
    /// limiter, so turning it up cannot make the output clip.
    pub fn set_master_volume(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 2.0) } else { 0.0 };
        self.master.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Current master fader position.
    pub fn master_volume(&self) -> f32 {
        f32::from_bits(self.master.load(Ordering::Relaxed))
    }

    /// Footsteps, without the caller holding a timer.
    ///
    /// Call once per frame with the block under the player's feet, their
    /// **horizontal** speed in blocks per second, whether they are on the
    /// ground, and the frame's `dt`. A step fires every [`FOOTSTEP_STRIDE`]
    /// blocks walked. Footsteps are always at the listener, so there is no
    /// position to pass.
    pub fn footstep(&self, ground: BlockId, speed: f32, on_ground: bool, dt: f32) {
        if !on_ground || ground.is_air() || !speed.is_finite() || speed < FOOTSTEP_MIN_SPEED {
            // Park part-way into the stride so walking again steps promptly.
            self.walked
                .store((FOOTSTEP_STRIDE * FOOTSTEP_PRIME).to_bits(), Ordering::Relaxed);
            return;
        }
        let mut d = f32::from_bits(self.walked.load(Ordering::Relaxed)) + speed * dt;
        if d >= FOOTSTEP_STRIDE {
            d = 0.0;
            self.play(Sound::Footstep(ground), PlayOpts::ui());
        }
        self.walked.store(d.to_bits(), Ordering::Relaxed);
    }

    /// Optional ambience, also without the caller holding a timer. Call once per
    /// frame; it fires a drip or a gust every [`AMBIENCE_MIN_GAP`]..
    /// [`AMBIENCE_MAX_GAP`] seconds. `underground` picks drips over wind.
    pub fn ambience(&self, listener: Vec3, underground: bool, dt: f32) {
        let mut left = f32::from_bits(self.ambience.load(Ordering::Relaxed)) - dt;
        if left <= 0.0 {
            let r = self.next_rand();
            let unit = (r >> 8) as f32 / 16_777_216.0;
            left = AMBIENCE_MIN_GAP + unit * (AMBIENCE_MAX_GAP - AMBIENCE_MIN_GAP);
            // Scatter it around the listener so it does not sound welded to
            // their head.
            let a = unit * std::f32::consts::TAU;
            let off = Vec3::new(a.cos(), (unit - 0.5) * 0.6, a.sin()) * AMBIENCE_DISTANCE;
            let sound = if underground { Sound::CaveDrip } else { Sound::Wind };
            self.play(sound, PlayOpts::at(listener + off, listener));
        }
        self.ambience.store(left.to_bits(), Ordering::Relaxed);
    }

    /// One step of a lock-free xorshift. Two threads racing here get the same
    /// number occasionally, which costs nothing but a repeated pitch.
    fn next_rand(&self) -> u32 {
        let mut x = self.rng.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng.store(x, Ordering::Relaxed);
        x
    }
}

/// Whether a sound kind that last played at `last_us` (0 meaning never) is
/// allowed to play again at `now_us`. This is the whole machine-gun defence:
/// the game loop can ask for a chip every frame and only some of them land.
fn cooldown_ready(last_us: u64, now_us: u64, gap_s: f32) -> bool {
    if last_us == 0 {
        return true;
    }
    let gap_us = if gap_s.is_finite() { (gap_s.max(0.0) * 1.0e6) as u64 } else { 0 };
    now_us >= last_us.saturating_add(gap_us)
}

impl Drop for Audio {
    fn drop(&mut self) {
        // Tell the engine thread to let the stream go. Deliberately not joined:
        // the main thread never waits on audio.
        self.shutdown.store(true, Ordering::Release);
    }
}

// -----------------------------------------------------------------------------
// Material families
// -----------------------------------------------------------------------------

/// The sound family a block belongs to. Callers never name one of these: they
/// pass a [`BlockId`] and [`Material::of`] does the mapping.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Material {
    Stone,
    Dirt,
    Sand,
    Gravel,
    Snow,
    Wood,
    Leaves,
    Metal,
    Crystal,
    Liquid,
}

/// Kept in the same order as the enum, because the bank indexes by `as usize`.
pub const MATERIALS: [Material; 10] = [
    Material::Stone,
    Material::Dirt,
    Material::Sand,
    Material::Gravel,
    Material::Snow,
    Material::Wood,
    Material::Leaves,
    Material::Metal,
    Material::Crystal,
    Material::Liquid,
];

impl Material {
    /// Which family a block sounds like. Anything unrecognised falls back to
    /// stone, which is the least surprising thing to hear.
    pub fn of(block: BlockId) -> Material {
        match block {
            BlockId::DIRT | BlockId::CLAY | BlockId::PODZOL => Material::Dirt,
            b if b.is_grassy() => Material::Dirt,
            BlockId::SAND => Material::Sand,
            BlockId::GRAVEL => Material::Gravel,
            BlockId::SNOW => Material::Snow,
            BlockId::WOOD
            | BlockId::PLANKS
            | BlockId::BIRCH_LOG
            | BlockId::SPRUCE_LOG
            | BlockId::CRAFTING_TABLE
            | BlockId::TORCH
            | BlockId::CACTUS => Material::Wood,
            BlockId::TALL_GRASS
            | BlockId::FLOWER_RED
            | BlockId::FLOWER_YELLOW
            | BlockId::DEAD_BUSH => Material::Leaves,
            b if b.is_leaves() => Material::Leaves,
            BlockId::COAL_ORE | BlockId::IRON_ORE | BlockId::GOLD_ORE => Material::Metal,
            BlockId::DIAMOND_ORE | BlockId::ICE => Material::Crystal,
            BlockId::WATER => Material::Liquid,
            _ => Material::Stone,
        }
    }

    /// This family's tuning-block entry.
    pub fn voice(self) -> MaterialVoice {
        match self {
            Material::Stone => VOICE_STONE,
            Material::Dirt => VOICE_DIRT,
            Material::Sand => VOICE_SAND,
            Material::Gravel => VOICE_GRAVEL,
            Material::Snow => VOICE_SNOW,
            Material::Wood => VOICE_WOOD,
            Material::Leaves => VOICE_LEAVES,
            Material::Metal => VOICE_METAL,
            Material::Crystal => VOICE_CRYSTAL,
            Material::Liquid => VOICE_LIQUID,
        }
    }
}

/// The four material-keyed actions. Same order as the bank layout.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Action {
    Dig,
    Break,
    Place,
    Step,
}

impl Action {
    fn shape(self) -> ActionShape {
        match self {
            Action::Dig => SHAPE_DIG,
            Action::Break => SHAPE_BREAK,
            Action::Place => SHAPE_PLACE,
            Action::Step => SHAPE_STEP,
        }
    }
}

const ACTIONS: [Action; 4] = [Action::Dig, Action::Break, Action::Place, Action::Step];

// -----------------------------------------------------------------------------
// Bank layout
// -----------------------------------------------------------------------------
//   Slot index is computed identically on the game thread (in `play`, to fill in
//   `Cmd::slot`) and on the audio thread (to index the rendered bank), so the
//   two never need to agree on anything but a number.

const MATERIAL_SLOTS: usize = MATERIALS.len() * ACTIONS.len() * VARIANTS;

/// The non-material sounds, in bank order.
const ONESHOTS: [Sound; 9] = [
    Sound::PlayerHurt,
    Sound::MobHurt,
    Sound::MobDeath,
    Sound::CreeperFuse,
    Sound::Explosion,
    Sound::BowShot,
    Sound::ArrowHit,
    Sound::CaveDrip,
    Sound::Wind,
];

const TOTAL_SLOTS: usize = MATERIAL_SLOTS + ONESHOTS.len() * VARIANTS;

/// One cooldown per sound kind, ignoring material.
const COOLDOWN_SLOTS: usize = 13;

fn material_slot(material: Material, action: Action, variant: usize) -> usize {
    ((material as usize * ACTIONS.len()) + action as usize) * VARIANTS + variant
}

fn oneshot_slot(which: usize, variant: usize) -> usize {
    MATERIAL_SLOTS + which * VARIANTS + variant
}

impl Sound {
    /// Index of this sound's first variant in the bank.
    fn slot_base(self) -> usize {
        match self {
            Sound::Dig(b) => material_slot(Material::of(b), Action::Dig, 0),
            Sound::Break(b) => material_slot(Material::of(b), Action::Break, 0),
            Sound::Place(b) => material_slot(Material::of(b), Action::Place, 0),
            Sound::Footstep(b) => material_slot(Material::of(b), Action::Step, 0),
            other => {
                let which = ONESHOTS.iter().position(|s| *s == other).unwrap_or(0);
                oneshot_slot(which, 0)
            }
        }
    }

    /// Nominal loudness, before `PlayOpts::volume`, distance and master.
    fn gain(self) -> f32 {
        match self {
            Sound::Dig(b) => GAIN_DIG * Material::of(b).voice().trim,
            Sound::Break(b) => GAIN_BREAK * Material::of(b).voice().trim,
            Sound::Place(b) => GAIN_PLACE * Material::of(b).voice().trim,
            Sound::Footstep(b) => GAIN_FOOTSTEP * Material::of(b).voice().trim,
            Sound::PlayerHurt => GAIN_PLAYER_HURT,
            Sound::MobHurt => GAIN_MOB_HURT,
            Sound::MobDeath => GAIN_MOB_DEATH,
            Sound::CreeperFuse => GAIN_FUSE,
            Sound::Explosion => GAIN_EXPLOSION,
            Sound::BowShot => GAIN_BOW,
            Sound::ArrowHit => GAIN_ARROW_HIT,
            Sound::CaveDrip => GAIN_DRIP,
            Sound::Wind => GAIN_WIND,
        }
    }

    fn cooldown(self) -> f32 {
        match self {
            Sound::Dig(_) => COOLDOWN_DIG,
            Sound::Break(_) => COOLDOWN_BREAK,
            Sound::Place(_) => COOLDOWN_PLACE,
            Sound::Footstep(_) => COOLDOWN_FOOTSTEP,
            Sound::PlayerHurt => COOLDOWN_PLAYER_HURT,
            Sound::MobHurt => COOLDOWN_MOB_HURT,
            Sound::MobDeath => COOLDOWN_MOB_DEATH,
            Sound::CreeperFuse => COOLDOWN_FUSE,
            Sound::Explosion => COOLDOWN_EXPLOSION,
            Sound::BowShot => COOLDOWN_BOW,
            Sound::ArrowHit => COOLDOWN_ARROW_HIT,
            Sound::CaveDrip | Sound::Wind => COOLDOWN_AMBIENCE,
        }
    }

    fn cooldown_slot(self) -> usize {
        match self {
            Sound::Dig(_) => 0,
            Sound::Break(_) => 1,
            Sound::Place(_) => 2,
            Sound::Footstep(_) => 3,
            Sound::PlayerHurt => 4,
            Sound::MobHurt => 5,
            Sound::MobDeath => 6,
            Sound::CreeperFuse => 7,
            Sound::Explosion => 8,
            Sound::BowShot => 9,
            Sound::ArrowHit => 10,
            Sound::CaveDrip => 11,
            Sound::Wind => 12,
        }
    }

    fn jitter(self) -> f32 {
        match self {
            Sound::Dig(_) => JITTER_DIG,
            Sound::Footstep(_) => JITTER_FOOTSTEP,
            Sound::Break(_) | Sound::Place(_) => JITTER_BREAK,
            Sound::PlayerHurt | Sound::MobHurt | Sound::MobDeath => JITTER_HURT,
            Sound::Explosion => JITTER_EXPLOSION,
            _ => JITTER_DEFAULT,
        }
    }
}

// -----------------------------------------------------------------------------
// Synthesis
// -----------------------------------------------------------------------------

/// xorshift32. Deterministic, so a given seed always renders the same buffer and
/// the tests can rely on it.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Rng(seed | 1)
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// White noise in -1..1.
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// Chamberlin state-variable filter. We only take the band-pass output, which is
/// the shape almost every impact sound in here is built from.
#[derive(Default)]
struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    /// One sample. `centre` may move between calls -- that is how the sweeps
    /// work. The coefficient is clamped for stability at high cutoffs.
    fn process(&mut self, x: f32, centre: f32, q: f32, sr: f32) -> f32 {
        let f = (2.0 * (std::f32::consts::PI * centre / sr).sin()).clamp(0.0, 1.0);
        let damp = (1.0 / q.max(0.4)).min(1.9);
        self.low += f * self.band;
        let high = x - self.low - damp * self.band;
        self.band += f * high;
        if !self.low.is_finite() || !self.band.is_finite() {
            self.low = 0.0;
            self.band = 0.0;
        }
        // Scale out the resonant gain so `q` changes colour, not level.
        self.band * damp
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Triangle wave from a phase in turns. Starts at zero, so it can open a buffer
/// without a step.
fn tri(turns: f32) -> f32 {
    let x = turns - turns.floor();
    let y = 4.0 * x;
    if y < 1.0 {
        y
    } else if y < 3.0 {
        2.0 - y
    } else {
        y - 4.0
    }
}

/// One synthesised impact: a swept band-passed noise burst with an optional
/// swept pitched body under it. Nearly every sound in the game is one of these.
#[derive(Copy, Clone, Debug)]
struct HitSpec {
    len: f32,
    attack: f32,
    /// Noise decay time constant.
    tau: f32,
    centre: f32,
    centre_end: f32,
    q: f32,
    grain: f32,
    noise_mix: f32,
    body: f32,
    body_end: f32,
    body_mix: f32,
    body_tau: f32,
    tri: bool,
}

impl HitSpec {
    /// Build one from a material voice and an action shape.
    fn from_material(v: MaterialVoice, s: ActionShape, detune: f32) -> Self {
        let centre = v.centre * s.centre_scale * detune;
        let body_mix = (v.body_mix * s.body_scale).clamp(0.0, 1.0);
        HitSpec {
            len: s.len,
            attack: s.attack,
            tau: s.tau,
            centre,
            centre_end: centre * v.sweep,
            q: v.q,
            grain: v.grain * detune,
            // A very body-heavy material still keeps some of its transient.
            noise_mix: 1.0 - 0.55 * body_mix,
            body: v.body * detune,
            body_end: v.body * detune,
            body_mix,
            body_tau: v.body_tau * s.body_tau_scale,
            tri: v.tri,
        }
    }
}

/// Render one impact into a fresh buffer, normalised to [`RENDER_PEAK`], silent
/// at both ends.
fn render_hit(spec: &HitSpec, sr: f32, seed: u32) -> Vec<f32> {
    let n = ((spec.len * sr).ceil() as usize).max(16);
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::new(seed);
    let mut svf = Svf::default();
    let mut phase = 0.0f32;
    let mut hold = 0.0f32;
    let mut hold_left = 0.0f32;
    let hold_period = if spec.grain > 1.0 { (sr / spec.grain).max(1.0) } else { 0.0 };

    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        let secs = i as f32 / sr;

        // Noise source, optionally sample-and-held into grains.
        let raw = if hold_period > 0.0 {
            if hold_left <= 0.0 {
                hold = rng.unit();
                hold_left = hold_period;
            }
            hold_left -= 1.0;
            hold
        } else {
            rng.unit()
        };

        let centre = lerp(spec.centre, spec.centre_end, t).clamp(20.0, sr * 0.45);
        let band = svf.process(raw, centre, spec.q, sr);
        let noise_env = (-secs / spec.tau.max(1.0e-4)).exp();

        let mut v = band * spec.noise_mix * noise_env;

        if spec.body_mix > 0.0 && spec.body > 0.0 {
            let f = lerp(spec.body, spec.body_end, t).max(1.0);
            phase += f / sr;
            if phase > 1.0 {
                phase -= phase.floor();
            }
            let osc = if spec.tri {
                tri(phase)
            } else {
                (phase * std::f32::consts::TAU).sin()
            };
            let body_env = (-secs / spec.body_tau.max(1.0e-4)).exp();
            v += osc * spec.body_mix * body_env;
        }

        *s = v;
    }

    finish(&mut out, spec.attack, sr);
    out
}

/// A creeper fuse: a hiss whose band rises as it goes, held rather than decayed.
fn render_fuse(sr: f32, seed: u32, detune: f32) -> Vec<f32> {
    let n = ((FUSE_LEN * sr).ceil() as usize).max(16);
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::new(seed);
    let mut svf = Svf::default();
    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        let centre = lerp(2600.0, 6200.0, t) * detune;
        // A slow flutter, so it sounds like something burning rather than a
        // noise generator being switched on.
        let flutter = 1.0 + 0.35 * (t * 37.0).sin();
        *s = svf.process(rng.unit(), centre.min(sr * 0.45), 0.8, sr) * flutter;
    }
    // Fade in and out so repeated triggers overlap into one continuous hiss.
    let edge = (0.09 * sr) as usize;
    for i in 0..edge.min(n / 2) {
        let k = i as f32 / edge as f32;
        out[i] *= k;
        out[n - 1 - i] *= k;
    }
    finish(&mut out, ATTACK_MIN, sr);
    out
}

/// The explosion: a noise transient over a low rumble that sweeps down.
fn render_explosion(sr: f32, seed: u32, detune: f32) -> Vec<f32> {
    let n = ((EXPLOSION_LEN * sr).ceil() as usize).max(16);
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::new(seed);
    let mut crack = Svf::default();
    let mut blast = Svf::default();
    let mut p_low = 0.0f32;
    let mut p_mid = 0.0f32;

    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        let secs = i as f32 / sr;
        let noise = rng.unit();

        // The initial crack: bright, gone in a few tens of milliseconds.
        let c = crack.process(noise, (4200.0 * detune).min(sr * 0.45), 0.7, sr);
        let v_crack = c * (-secs / 0.030).exp() * 0.60;

        // The blast body: broadband, sweeping down as it decays.
        let bc = lerp(900.0, 130.0, t.min(0.5) * 2.0) * detune;
        let b = blast.process(noise, bc.clamp(20.0, sr * 0.45), 0.9, sr);
        let v_blast = b * (-secs / 0.34).exp();

        // The rumble: two low sines an interval apart so they beat against each
        // other instead of sounding like a test tone.
        let f_low = lerp(95.0, 32.0, t.min(0.7) / 0.7) * detune;
        let f_mid = lerp(148.0, 58.0, t.min(0.7) / 0.7) * detune;
        p_low = (p_low + f_low / sr).fract();
        p_mid = (p_mid + f_mid / sr).fract();
        let rumble = ((p_low * std::f32::consts::TAU).sin() * 0.9
            + (p_mid * std::f32::consts::TAU).sin() * 0.5)
            * (-secs / 0.55).exp();

        *s = v_crack + v_blast + rumble * 0.9;
    }

    finish(&mut out, ATTACK_MIN, sr);
    out
}

/// A cave drip: a short plink whose pitch rises. The rise is the whole illusion.
fn render_drip(sr: f32, seed: u32, detune: f32) -> Vec<f32> {
    let spec = HitSpec {
        len: DRIP_LEN,
        attack: 0.0015,
        tau: 0.010,
        centre: 2600.0 * detune,
        centre_end: 4200.0 * detune,
        q: 1.4,
        grain: 0.0,
        noise_mix: 0.25,
        body: 760.0 * detune,
        body_end: 1560.0 * detune,
        body_mix: 0.90,
        body_tau: 0.055,
        tri: false,
    };
    render_hit(&spec, sr, seed)
}

/// Wind: low-passed noise with a slowly wandering cutoff, faded in and out.
fn render_wind(sr: f32, seed: u32, detune: f32) -> Vec<f32> {
    let n = ((WIND_LEN * sr).ceil() as usize).max(16);
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::new(seed);
    let mut y = 0.0f32;
    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        // Two slow, unrelated LFOs: one on brightness, one on level.
        let lfo_a = (t * 2.3 * std::f32::consts::TAU).sin();
        let lfo_b = (t * 1.1 * std::f32::consts::TAU + 1.7).sin();
        let cutoff = (420.0 * detune) * (1.0 + 0.55 * lfo_a);
        let a = (cutoff / (cutoff + sr / std::f32::consts::TAU)).clamp(0.0005, 0.9);
        y += (rng.unit() - y) * a;
        *s = y * (0.55 + 0.45 * lfo_b);
    }
    // A long fade at both ends: a gust arrives and leaves, it does not switch on.
    let edge = (0.6 * sr) as usize;
    for i in 0..edge.min(n / 2) {
        let k = i as f32 / edge as f32;
        let k = k * k * (3.0 - 2.0 * k);
        out[i] *= k;
        out[n - 1 - i] *= k;
    }
    finish(&mut out, ATTACK_MIN, sr);
    out
}

/// Shared tail of every render: attack ramp, fade to exact zero, normalise.
/// After this the buffer starts at 0, ends at 0, and peaks at [`RENDER_PEAK`].
fn finish(buf: &mut [f32], attack: f32, sr: f32) {
    let n = buf.len();
    if n == 0 {
        return;
    }

    // Any non-finite sample from a pathological filter state becomes silence
    // rather than a burst of noise at full scale.
    for s in buf.iter_mut() {
        if !s.is_finite() {
            *s = 0.0;
        }
    }

    // Attack: smoothstep up, so sample 0 is exactly zero.
    let a = ((attack.max(ATTACK_MIN) * sr) as usize).clamp(2, n);
    for (i, s) in buf.iter_mut().take(a).enumerate() {
        let k = i as f32 / a as f32;
        *s *= k * k * (3.0 - 2.0 * k);
    }

    // Tail: linear down to exactly zero on the last sample.
    let f = ((TAIL_FADE * sr) as usize).clamp(2, n);
    for i in 0..f {
        let k = i as f32 / f as f32;
        buf[n - 1 - i] *= k;
    }

    // Normalise. A silent buffer is left alone rather than amplified.
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1.0e-6 {
        let k = RENDER_PEAK / peak;
        for s in buf.iter_mut() {
            *s *= k;
        }
    }
}

/// Every rendered sound, indexed by slot.
pub struct Bank {
    slots: Vec<Vec<f32>>,
}

impl Bank {
    /// Render the whole bank at the device's sample rate. Costs a few
    /// milliseconds and happens once, on the audio thread, before the stream
    /// starts -- never on the main thread.
    pub fn render(sr: f32) -> Bank {
        let sr = if sr.is_finite() && sr >= 8000.0 { sr } else { 44_100.0 };
        let mut slots = vec![Vec::new(); TOTAL_SLOTS];

        for &m in MATERIALS.iter() {
            for &action in ACTIONS.iter() {
                for variant in 0..VARIANTS {
                    let detune = variant_detune(variant);
                    let spec = HitSpec::from_material(m.voice(), action.shape(), detune);
                    let seed = seed_for(m as u32 * 97 + action as u32 * 13, variant);
                    slots[material_slot(m, action, variant)] = render_hit(&spec, sr, seed);
                }
            }
        }

        for (which, &sound) in ONESHOTS.iter().enumerate() {
            for variant in 0..VARIANTS {
                let detune = variant_detune(variant);
                let seed = seed_for(1000 + which as u32 * 31, variant);
                slots[oneshot_slot(which, variant)] = render_oneshot(sound, sr, seed, detune);
            }
        }

        Bank { slots }
    }

    fn get(&self, slot: usize) -> Option<&Vec<f32>> {
        self.slots.get(slot)
    }

    /// Number of slots. Every one is populated after `render`.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Never true after `render`; present so `len` does not stand alone.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

fn variant_detune(variant: usize) -> f32 {
    // -VARIANT_DETUNE, 0, +VARIANT_DETUNE for three variants.
    let half = (VARIANTS as f32 - 1.0) * 0.5;
    1.0 + (variant as f32 - half) * VARIANT_DETUNE
}

fn seed_for(base: u32, variant: usize) -> u32 {
    (base.wrapping_mul(0x9E37_79B9) ^ ((variant as u32 + 1).wrapping_mul(0x85EB_CA6B))) | 1
}

/// The non-material sounds. Each is a `HitSpec` unless it needs layers.
fn render_oneshot(sound: Sound, sr: f32, seed: u32, detune: f32) -> Vec<f32> {
    match sound {
        // A grunt: mostly a pitched body sweeping down, with breath over it.
        Sound::PlayerHurt => render_hit(
            &HitSpec {
                len: HURT_LEN,
                attack: 0.006,
                tau: 0.060,
                centre: 900.0 * detune,
                centre_end: 500.0 * detune,
                q: 1.1,
                grain: 0.0,
                noise_mix: 0.45,
                body: 210.0 * detune,
                body_end: 148.0 * detune,
                body_mix: 0.80,
                body_tau: 0.090,
                tri: false,
            },
            sr,
            seed,
        ),
        // Higher and shorter than the player's, so the two never blur together.
        Sound::MobHurt => render_hit(
            &HitSpec {
                len: 0.22,
                attack: 0.004,
                tau: 0.045,
                centre: 1300.0 * detune,
                centre_end: 760.0 * detune,
                q: 1.3,
                grain: 0.0,
                noise_mix: 0.50,
                body: 330.0 * detune,
                body_end: 262.0 * detune,
                body_mix: 0.75,
                body_tau: 0.060,
                tri: true,
            },
            sr,
            seed,
        ),
        // The same voice, falling much further and lasting much longer.
        Sound::MobDeath => render_hit(
            &HitSpec {
                len: DEATH_LEN,
                attack: 0.008,
                tau: 0.130,
                centre: 1150.0 * detune,
                centre_end: 420.0 * detune,
                q: 1.2,
                grain: 0.0,
                noise_mix: 0.45,
                body: 300.0 * detune,
                body_end: 105.0 * detune,
                body_mix: 0.80,
                body_tau: 0.200,
                tri: true,
            },
            sr,
            seed,
        ),
        Sound::CreeperFuse => render_fuse(sr, seed, detune),
        Sound::Explosion => render_explosion(sr, seed, detune),
        // A string release: a bright whoosh collapsing fast onto a low twang.
        Sound::BowShot => render_hit(
            &HitSpec {
                len: BOW_LEN,
                attack: 0.0015,
                tau: 0.055,
                centre: 4600.0 * detune,
                centre_end: 700.0 * detune,
                q: 1.0,
                grain: 0.0,
                noise_mix: 0.85,
                body: 210.0 * detune,
                body_end: 150.0 * detune,
                body_mix: 0.35,
                body_tau: 0.045,
                tri: true,
            },
            sr,
            seed,
        ),
        // A thock. Short, mid, no ring.
        Sound::ArrowHit => render_hit(
            &HitSpec {
                len: ARROW_HIT_LEN,
                attack: 0.0012,
                tau: 0.018,
                centre: 1400.0 * detune,
                centre_end: 900.0 * detune,
                q: 2.0,
                grain: 0.0,
                noise_mix: 0.70,
                body: 360.0 * detune,
                body_end: 300.0 * detune,
                body_mix: 0.45,
                body_tau: 0.030,
                tri: true,
            },
            sr,
            seed,
        ),
        Sound::CaveDrip => render_drip(sr, seed, detune),
        Sound::Wind => render_wind(sr, seed, detune),
        // Material sounds never reach here; a stone dig is a harmless stand-in.
        _ => render_hit(
            &HitSpec::from_material(VOICE_STONE, SHAPE_DIG, detune),
            sr,
            seed,
        ),
    }
}

// -----------------------------------------------------------------------------
// Mixing
// -----------------------------------------------------------------------------

/// What the game thread sends the audio thread. Copy, 12 bytes, no allocation.
#[derive(Copy, Clone, Debug)]
struct Cmd {
    slot: u16,
    rate: f32,
    gain: f32,
}

/// One playing sound. `gain <= 0.0` means the slot is free.
#[derive(Copy, Clone, Debug, Default)]
struct Voice {
    slot: usize,
    pos: f32,
    rate: f32,
    gain: f32,
}

/// The voice pool. Testable on its own: no device, no channel, no cpal.
pub struct Mixer {
    bank: Bank,
    voices: Vec<Voice>,
}

impl Mixer {
    pub fn new(bank: Bank) -> Mixer {
        Mixer { bank, voices: vec![Voice::default(); MAX_VOICES] }
    }

    /// Start a sound, stealing the quietest playing voice if the pool is full.
    fn trigger(&mut self, cmd: Cmd) {
        let slot = cmd.slot as usize;
        if self.bank.get(slot).is_none_or(|b| b.len() < 2) {
            return;
        }
        let idx = match self.voices.iter().position(|v| v.gain <= 0.0) {
            Some(i) => i,
            None => {
                let mut worst = 0;
                for (i, v) in self.voices.iter().enumerate() {
                    if v.gain < self.voices[worst].gain {
                        worst = i;
                    }
                }
                worst
            }
        };
        self.voices[idx] = Voice { slot, pos: 0.0, rate: cmd.rate, gain: cmd.gain };
    }

    /// Sum every playing voice into `out`, apply `master`, then limit.
    ///
    /// The output is guaranteed to be finite and strictly inside (-1, 1) no
    /// matter how many voices are playing or how loud they are.
    pub fn render_mono(&mut self, out: &mut [f32], master: f32) {
        out.fill(0.0);
        let master = if master.is_finite() { master.clamp(0.0, 4.0) } else { 0.0 };

        // Split the borrow: the bank is read while the voices are advanced.
        let Mixer { bank, voices } = self;
        for v in voices.iter_mut() {
            if v.gain <= 0.0 {
                continue;
            }
            let Some(buf) = bank.get(v.slot) else {
                v.gain = 0.0;
                continue;
            };
            let n = buf.len();
            for s in out.iter_mut() {
                let i = v.pos as usize;
                if i + 1 >= n {
                    v.gain = 0.0;
                    break;
                }
                // Linear interpolation: the resampler *is* the pitch shifter.
                let frac = v.pos - i as f32;
                *s += (buf[i] + (buf[i + 1] - buf[i]) * frac) * v.gain;
                v.pos += v.rate;
            }
        }

        for s in out.iter_mut() {
            *s = soft_clip(*s * master);
        }
    }

    /// How many voices are currently sounding. Diagnostics and tests.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.gain > 0.0).count()
    }
}

/// Transparent below [`LIMITER_KNEE`], asymptotic to [`LIMITER_CEILING`] above
/// it. Continuous and slope-matched at the knee, so the transition itself is
/// inaudible, and mathematically incapable of returning a value outside
/// [-LIMITER_CEILING, LIMITER_CEILING] -- however many voices pile up.
pub fn soft_clip(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let a = x.abs();
    if a <= LIMITER_KNEE {
        return x;
    }
    let room = LIMITER_CEILING - LIMITER_KNEE;
    let over = (a - LIMITER_KNEE) / room;
    let y = LIMITER_KNEE + room * (1.0 - (-over).exp());
    if x < 0.0 { -y } else { y }
}

// -----------------------------------------------------------------------------
// The audio thread
// -----------------------------------------------------------------------------

mod engine {
    use super::{Audio, Bank, Cmd, MAX_VOICES, Mixer};
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// How long the owner thread sleeps between shutdown checks.
    const PARK: Duration = Duration::from_millis(120);

    /// Everything the stream callback owns.
    struct Engine {
        mixer: Mixer,
        rx: Receiver<Cmd>,
        master: Arc<AtomicU32>,
        scratch: Vec<f32>,
    }

    impl Engine {
        fn fill<T>(&mut self, data: &mut [T], channels: usize)
        where
            T: cpal::Sample + cpal::FromSample<f32>,
        {
            while let Ok(cmd) = self.rx.try_recv() {
                self.mixer.trigger(cmd);
            }
            let channels = channels.max(1);
            let frames = data.len() / channels;
            if self.scratch.len() < frames {
                // Only ever happens if the backend hands us a bigger period than
                // we reserved; after the first time it never allocates again.
                self.scratch.resize(frames, 0.0);
            }
            let master = f32::from_bits(self.master.load(Ordering::Relaxed));
            self.mixer.render_mono(&mut self.scratch[..frames], master);
            for (frame, chunk) in data.chunks_mut(channels).enumerate() {
                let v = T::from_sample(self.scratch[frame]);
                for c in chunk.iter_mut() {
                    *c = v;
                }
            }
        }
    }

    /// Thread body. Wrapped so that a panic anywhere in a platform backend costs
    /// the player sound rather than the process.
    pub(super) fn run_guarded(
        rx: Receiver<Cmd>,
        master: Arc<AtomicU32>,
        shutdown: Arc<AtomicBool>,
    ) {
        let out = std::panic::catch_unwind(AssertUnwindSafe(|| run(rx, master, shutdown)));
        if out.is_err() {
            eprintln!("[loudstone] audio: the audio thread failed -- continuing in silence");
        }
    }

    fn run(rx: Receiver<Cmd>, master: Arc<AtomicU32>, shutdown: Arc<AtomicBool>) {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            eprintln!("[loudstone] audio: no output device -- the game runs silent");
            return;
        };
        let supported = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[loudstone] audio: device has no usable output config ({e}) -- silent");
                return;
            }
        };

        let sample_rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let format = supported.sample_format();
        let config = supported.config();

        // Rendered here, on this thread, so the main thread never pays for it.
        let bank = Bank::render(sample_rate as f32);
        let engine = Engine {
            mixer: Mixer::new(bank),
            rx,
            master,
            // Generous enough that the callback never allocates in practice.
            scratch: vec![0.0; 8192],
        };

        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, engine, channels),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, engine, channels),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, engine, channels),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, engine, channels),
            cpal::SampleFormat::F64 => build::<f64>(&device, config, engine, channels),
            other => {
                eprintln!("[loudstone] audio: unsupported sample format {other:?} -- silent");
                return;
            }
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[loudstone] audio: could not open the output stream ({e}) -- silent");
                return;
            }
        };
        if let Err(e) = stream.play() {
            eprintln!("[loudstone] audio: could not start the output stream ({e}) -- silent");
            return;
        }

        println!(
            "[loudstone] audio: {device} @ {sample_rate} Hz, {channels} ch, {format:?}, {MAX_VOICES} voices"
        );

        // The stream lives as long as this thread does; cpal streams stop when
        // they are dropped, so the thread simply waits to be told to let go.
        while !shutdown.load(Ordering::Acquire) {
            std::thread::sleep(PARK);
        }
        drop(stream);
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut engine: Engine,
        channels: usize,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        device.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| engine.fill(data, channels),
            |e| eprintln!("[loudstone] audio: stream error: {e}"),
            None,
        )
    }

    /// Compile-time proof that `Audio` can be shared across threads, which is
    /// what makes `play(&self)` callable from anywhere in the game.
    const _: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Audio>();
    };
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------
//
// Nobody involved in writing this could hear it, so the tests check everything
// that can be checked without ears: buffers are the right length, finite, inside
// [-1, 1], silent at both ends, and different between materials; mixing a full
// pool of voices at full gain still cannot clip; and a machine with no sound
// card gets a working stub instead of a panic.

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44_100.0;

    fn bank() -> Bank {
        Bank::render(SR)
    }

    fn every_slot(b: &Bank) -> impl Iterator<Item = (usize, &Vec<f32>)> {
        b.slots.iter().enumerate()
    }

    #[test]
    fn bank_is_completely_populated() {
        let b = bank();
        assert_eq!(b.len(), TOTAL_SLOTS);
        assert!(!b.is_empty());
        for (i, buf) in every_slot(&b) {
            assert!(buf.len() > 64, "slot {i} is only {} samples", buf.len());
        }
    }

    #[test]
    fn every_buffer_is_finite_and_inside_unity() {
        for (i, buf) in every_slot(&bank()) {
            for (j, s) in buf.iter().enumerate() {
                assert!(s.is_finite(), "slot {i} sample {j} is {s}");
                assert!(
                    s.abs() <= 1.0,
                    "slot {i} sample {j} is {s}: that clips on its own"
                );
            }
        }
    }

    #[test]
    fn every_buffer_is_normalised_to_the_render_peak() {
        for (i, buf) in every_slot(&bank()) {
            let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(
                (peak - RENDER_PEAK).abs() < 1.0e-4,
                "slot {i} peaks at {peak}, expected {RENDER_PEAK}"
            );
        }
    }

    #[test]
    fn every_buffer_starts_and_ends_silent() {
        // A step at either end of a buffer is an audible click every time the
        // sound plays, which is the most likely way this module sounds broken.
        for (i, buf) in every_slot(&bank()) {
            let n = buf.len();
            assert_eq!(buf[0], 0.0, "slot {i} starts at {}", buf[0]);
            assert_eq!(buf[n - 1], 0.0, "slot {i} ends at {}", buf[n - 1]);
            // And the approach to zero is gradual, not a single-sample drop.
            let head = buf[..8].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let tail = buf[n - 8..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(head < 0.35, "slot {i} opens with a step ({head})");
            assert!(tail < 0.35, "slot {i} closes with a step ({tail})");
        }
    }

    #[test]
    fn buffer_lengths_match_the_tuning_block() {
        let b = bank();
        for &m in MATERIALS.iter() {
            for &a in ACTIONS.iter() {
                let want = (a.shape().len * SR).ceil() as usize;
                let got = b.slots[material_slot(m, a, 0)].len();
                assert_eq!(got, want, "{m:?}/{a:?} is {got} samples, expected {want}");
            }
        }
        let expl = b.slots[oneshot_slot(4, 0)].len();
        assert_eq!(expl, (EXPLOSION_LEN * SR).ceil() as usize);
    }

    #[test]
    fn materials_sound_different_from_each_other() {
        let b = bank();
        // Compare the mean absolute difference of the first 40 ms of each
        // material's dig. Two families that render identically would be a
        // mapping bug, and would make mining every block sound the same.
        let n = (0.04 * SR) as usize;
        for (i, &a) in MATERIALS.iter().enumerate() {
            for &c in MATERIALS.iter().skip(i + 1) {
                let x = &b.slots[material_slot(a, Action::Dig, 0)];
                let y = &b.slots[material_slot(c, Action::Dig, 0)];
                let d: f32 = x[..n]
                    .iter()
                    .zip(y[..n].iter())
                    .map(|(p, q)| (p - q).abs())
                    .sum::<f32>()
                    / n as f32;
                assert!(d > 0.01, "{a:?} and {c:?} dig almost identically ({d})");
            }
        }
    }

    #[test]
    fn variants_of_one_sound_differ() {
        let b = bank();
        let x = &b.slots[material_slot(Material::Stone, Action::Dig, 0)];
        let y = &b.slots[material_slot(Material::Stone, Action::Dig, 1)];
        let n = x.len().min(y.len());
        let d: f32 = x[..n].iter().zip(y[..n].iter()).map(|(p, q)| (p - q).abs()).sum::<f32>()
            / n as f32;
        assert!(d > 0.01, "the stone dig variants are the same sound ({d})");
    }

    #[test]
    fn actions_differ_within_one_material() {
        let b = bank();
        let dig = &b.slots[material_slot(Material::Stone, Action::Dig, 0)];
        let brk = &b.slots[material_slot(Material::Stone, Action::Break, 0)];
        assert!(brk.len() > dig.len() * 2, "a break should outlast a chip");
    }

    #[test]
    fn a_full_pool_of_loud_voices_cannot_clip() {
        // The defect this catches is the obvious one: several sounds landing on
        // the same frame and summing past full scale.
        let mut mx = Mixer::new(bank());
        for i in 0..MAX_VOICES * 2 {
            mx.trigger(Cmd {
                slot: (i % TOTAL_SLOTS) as u16,
                rate: 1.0,
                gain: 1.0,
            });
        }
        assert_eq!(mx.active_voices(), MAX_VOICES);
        let mut out = vec![0.0f32; 4096];
        let mut loudest = 0.0f32;
        for _ in 0..8 {
            // Master at 2.0 as well, i.e. the worst case the API allows.
            mx.render_mono(&mut out, 2.0);
            for (i, s) in out.iter().enumerate() {
                assert!(s.is_finite(), "sample {i} is {s}");
                assert!(s.abs() <= LIMITER_CEILING, "sample {i} clipped at {s}");
                loudest = loudest.max(s.abs());
            }
        }
        // And it really did have to work for it: a mix that never approached the
        // ceiling would prove nothing.
        assert!(loudest > LIMITER_KNEE, "the pool never got loud enough to test");
    }

    #[test]
    fn the_limiter_is_transparent_below_the_knee_and_bounded_above_it() {
        for k in -2000..=2000 {
            let x = k as f32 * 0.01;
            let y = soft_clip(x);
            assert!(y.abs() <= LIMITER_CEILING, "soft_clip({x}) = {y}");
            if x.abs() <= LIMITER_KNEE {
                assert_eq!(y, x, "soft_clip should not touch {x}");
            }
        }
        assert_eq!(soft_clip(f32::NAN), 0.0);
        assert_eq!(soft_clip(f32::INFINITY), 0.0);
        assert!(soft_clip(1000.0) <= LIMITER_CEILING);
        assert!(soft_clip(-1000.0) >= -LIMITER_CEILING);
        assert!(soft_clip(f32::MAX) <= LIMITER_CEILING);
    }

    #[test]
    fn a_single_voice_actually_makes_sound() {
        // The bug this module exists to fix is "the game makes no sound", so it
        // is worth asserting that one voice at its tuned gain produces real
        // signal and not a buffer of zeros.
        let mut mx = Mixer::new(bank());
        mx.trigger(Cmd {
            slot: material_slot(Material::Stone, Action::Break, 0) as u16,
            rate: 1.0,
            gain: GAIN_BREAK,
        });
        let mut out = vec![0.0f32; 4096];
        mx.render_mono(&mut out, DEFAULT_MASTER_VOLUME);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(rms > 0.02, "a block break at nominal gain is inaudible (rms {rms})");
        assert!(peak > 0.10, "a block break barely moves the cone (peak {peak})");
        assert!(peak <= LIMITER_CEILING, "one voice alone should not limit ({peak})");
    }

    #[test]
    fn the_dig_cooldown_throttles_the_mining_tick() {
        // Chipping dirt retriggers every 36 ms. Run two seconds of that through
        // the same gate `play` uses and count what would actually be heard.
        let mut last = 0u64;
        let mut plays = 0;
        let mut t = 0u64;
        while t < 2_000_000 {
            if cooldown_ready(last, t, COOLDOWN_DIG) {
                last = t.max(1);
                plays += 1;
            }
            t += 36_000;
        }
        let per_second = plays as f32 / 2.0;
        assert!(per_second <= 13.0, "{per_second} ticks a second is a buzz");
        assert!(per_second >= 7.0, "{per_second} ticks a second reads as broken, not slow");
    }

    #[test]
    fn the_cooldown_gate_handles_its_edges() {
        assert!(cooldown_ready(0, 0, 1.0), "the first play must always land");
        assert!(!cooldown_ready(1, 1, 1.0));
        assert!(cooldown_ready(1, 1_000_001, 1.0));
        assert!(cooldown_ready(1, u64::MAX, 1.0));
        assert!(cooldown_ready(u64::MAX, u64::MAX, 0.0));
        // A nonsense gap must not lock a sound out forever.
        assert!(cooldown_ready(1, 2, f32::NAN));
        assert!(cooldown_ready(1, 2, -5.0));
    }

    #[test]
    fn voices_free_themselves_when_they_finish() {
        let mut mx = Mixer::new(bank());
        mx.trigger(Cmd { slot: material_slot(Material::Stone, Action::Dig, 0) as u16, rate: 1.0, gain: 0.5 });
        assert_eq!(mx.active_voices(), 1);
        let mut out = vec![0.0f32; 8192]; // longer than any dig buffer
        mx.render_mono(&mut out, 1.0);
        assert_eq!(mx.active_voices(), 0, "a finished voice must free its slot");
    }

    #[test]
    fn a_bad_slot_is_ignored_rather_than_panicking() {
        let mut mx = Mixer::new(bank());
        mx.trigger(Cmd { slot: u16::MAX, rate: 1.0, gain: 1.0 });
        assert_eq!(mx.active_voices(), 0);
        let mut out = vec![0.0f32; 256];
        mx.render_mono(&mut out, 1.0);
    }

    #[test]
    fn pitch_shifting_stays_in_range_and_terminates() {
        let mut mx = Mixer::new(bank());
        for rate in [PITCH_MIN, 0.5, 1.0, 2.0, PITCH_MAX] {
            mx.trigger(Cmd {
                slot: material_slot(Material::Wood, Action::Break, 0) as u16,
                rate,
                gain: 1.0,
            });
        }
        let mut out = vec![0.0f32; 1024];
        // 0.38 s of wood at quarter speed is 1.52 s; 80 blocks of 1024 samples
        // at 44.1 kHz is 1.86 s, so every voice must have retired by then.
        for _ in 0..80 {
            mx.render_mono(&mut out, 1.0);
            for s in out.iter() {
                assert!(s.is_finite() && s.abs() < 1.0);
            }
        }
        assert_eq!(mx.active_voices(), 0);
    }

    #[test]
    fn every_block_maps_to_a_material_and_a_real_slot() {
        let b = bank();
        for n in 0..=BlockId::MAX {
            let id = BlockId(n);
            for s in [
                Sound::Dig(id),
                Sound::Break(id),
                Sound::Place(id),
                Sound::Footstep(id),
            ] {
                let base = s.slot_base();
                assert!(base + VARIANTS <= TOTAL_SLOTS, "{s:?} indexes off the end");
                for v in 0..VARIANTS {
                    assert!(!b.slots[base + v].is_empty(), "{s:?} variant {v} is empty");
                }
                assert!(s.gain() > 0.0);
                assert!(s.cooldown() >= 0.0);
            }
        }
    }

    #[test]
    fn the_material_mapping_is_the_one_we_meant() {
        assert_eq!(Material::of(BlockId::STONE), Material::Stone);
        assert_eq!(Material::of(BlockId::COBBLESTONE), Material::Stone);
        assert_eq!(Material::of(BlockId::GRANITE), Material::Stone);
        assert_eq!(Material::of(BlockId::DIRT), Material::Dirt);
        assert_eq!(Material::of(BlockId::GRASS), Material::Dirt);
        assert_eq!(Material::of(BlockId::GRASS_SWAMP), Material::Dirt);
        assert_eq!(Material::of(BlockId::PODZOL), Material::Dirt);
        assert_eq!(Material::of(BlockId::SAND), Material::Sand);
        assert_eq!(Material::of(BlockId::GRAVEL), Material::Gravel);
        assert_eq!(Material::of(BlockId::SNOW), Material::Snow);
        assert_eq!(Material::of(BlockId::WOOD), Material::Wood);
        assert_eq!(Material::of(BlockId::PLANKS), Material::Wood);
        assert_eq!(Material::of(BlockId::SPRUCE_LOG), Material::Wood);
        assert_eq!(Material::of(BlockId::CRAFTING_TABLE), Material::Wood);
        assert_eq!(Material::of(BlockId::LEAVES), Material::Leaves);
        assert_eq!(Material::of(BlockId::BIRCH_LEAVES), Material::Leaves);
        assert_eq!(Material::of(BlockId::TALL_GRASS), Material::Leaves);
        assert_eq!(Material::of(BlockId::IRON_ORE), Material::Metal);
        assert_eq!(Material::of(BlockId::COAL_ORE), Material::Metal);
        assert_eq!(Material::of(BlockId::DIAMOND_ORE), Material::Crystal);
        assert_eq!(Material::of(BlockId::ICE), Material::Crystal);
        assert_eq!(Material::of(BlockId::WATER), Material::Liquid);
        // Anything unknown must still make a sound rather than panic.
        assert_eq!(Material::of(BlockId(200)), Material::Stone);
    }

    #[test]
    fn every_slot_is_reachable_from_some_sound() {
        // If a slot is rendered but nothing can ever play it, the layout maths
        // has drifted from the enum.
        let mut hit = [false; TOTAL_SLOTS];
        for &m in MATERIALS.iter() {
            for &a in ACTIONS.iter() {
                for v in 0..VARIANTS {
                    hit[material_slot(m, a, v)] = true;
                }
            }
        }
        for (i, _) in ONESHOTS.iter().enumerate() {
            for v in 0..VARIANTS {
                hit[oneshot_slot(i, v)] = true;
            }
        }
        assert!(hit.iter().all(|&h| h), "some bank slots are unreachable");
    }

    #[test]
    fn one_shots_land_on_their_own_slots() {
        for (i, &s) in ONESHOTS.iter().enumerate() {
            assert_eq!(s.slot_base(), oneshot_slot(i, 0), "{s:?} is misfiled");
        }
        // Cooldown slots must be unique, or two sounds throttle each other.
        let mut seen = [false; COOLDOWN_SLOTS];
        let mut all: Vec<Sound> = ONESHOTS.to_vec();
        all.extend([
            Sound::Dig(BlockId::STONE),
            Sound::Break(BlockId::STONE),
            Sound::Place(BlockId::STONE),
            Sound::Footstep(BlockId::STONE),
        ]);
        for s in all {
            let slot = s.cooldown_slot();
            assert!(!seen[slot], "{s:?} shares a cooldown slot");
            seen[slot] = true;
        }
    }

    #[test]
    fn the_silent_stub_works_and_plays_nothing() {
        let a = Audio::silent();
        assert!(!a.is_enabled());
        // Every call must be a harmless no-op, including the daft ones.
        a.play(Sound::Dig(BlockId::STONE), PlayOpts::default());
        a.play(Sound::Explosion, PlayOpts::at(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0)));
        a.play(Sound::Wind, PlayOpts::default().with_volume(f32::NAN));
        a.play(Sound::MobHurt, PlayOpts::at(Vec3::NAN, Vec3::ZERO));
        a.play(Sound::BowShot, PlayOpts::default().with_pitch(0.0));
        a.play(Sound::PlayerHurt, PlayOpts::default().with_pitch(f32::INFINITY));
        a.footstep(BlockId::GRASS, 4.0, true, 0.016);
        a.footstep(BlockId::AIR, 4.0, false, 0.016);
        a.ambience(Vec3::ZERO, true, 100.0);
        a.set_master_volume(0.5);
        assert_eq!(a.master_volume(), 0.5);
        a.set_master_volume(f32::NAN);
        assert_eq!(a.master_volume(), 0.0);
        a.set_master_volume(99.0);
        assert_eq!(a.master_volume(), 2.0);
    }

    #[test]
    fn constructing_and_dropping_audio_never_panics() {
        // On a machine with no sound card this takes the silent path; on one
        // with a device it opens the real stream. Neither may panic, and
        // neither may block for any meaningful time.
        let t = Instant::now();
        let a = Audio::new();
        assert!(
            t.elapsed().as_millis() < 250,
            "Audio::new blocked the caller for {:?}",
            t.elapsed()
        );
        for _ in 0..500 {
            a.play(Sound::Dig(BlockId::STONE), PlayOpts::at(Vec3::ZERO, Vec3::ZERO));
        }
        a.set_master_volume(0.0);
        drop(a);
    }

    #[test]
    fn distance_and_cooldown_reject_before_the_queue() {
        // The stub cannot observe sends, so this checks the arithmetic the
        // rejection rests on rather than the send itself.
        let far = Vec3::new(MAX_AUDIBLE_DISTANCE + 1.0, 0.0, 0.0);
        assert!(far.distance_squared(Vec3::ZERO) > MAX_AUDIBLE_DISTANCE * MAX_AUDIBLE_DISTANCE);
        let att = |d: f32| 1.0 / (1.0 + d * d / (ATTENUATION_HALF_DISTANCE * ATTENUATION_HALF_DISTANCE));
        assert!((att(0.0) - 1.0).abs() < 1.0e-6);
        assert!((att(ATTENUATION_HALF_DISTANCE) - 0.5).abs() < 1.0e-6);
        assert!(att(MAX_AUDIBLE_DISTANCE) * GAIN_BREAK < GAIN_BREAK * 0.05);
    }

    #[test]
    fn every_material_voice_is_sane() {
        // The scalar guard rails on the tuning block are `const _: () = assert!`
        // up at the top of the file, so they fail the build rather than a test.
        // The tables need a loop, which const eval will not do.
        for &m in MATERIALS.iter() {
            let v = m.voice();
            assert!(v.centre > 20.0 && v.q > 0.0 && v.trim > 0.0, "{m:?}");
        }
    }

    #[test]
    fn variant_detune_is_centred() {
        let sum: f32 = (0..VARIANTS).map(variant_detune).sum();
        assert!((sum - VARIANTS as f32).abs() < 1.0e-5, "detune is lopsided");
        for v in 0..VARIANTS {
            assert!(variant_detune(v) > 0.5);
        }
    }

    #[test]
    fn a_daft_sample_rate_still_renders() {
        for sr in [8000.0, 22_050.0, 48_000.0, 96_000.0, 192_000.0, f32::NAN, 0.0] {
            let b = Bank::render(sr);
            assert_eq!(b.len(), TOTAL_SLOTS);
            for (i, buf) in every_slot(&b) {
                assert!(!buf.is_empty(), "slot {i} empty at {sr} Hz");
                for s in buf.iter() {
                    assert!(s.is_finite() && s.abs() <= 1.0, "slot {i} at {sr} Hz: {s}");
                }
            }
        }
    }
}
