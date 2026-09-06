# Mob and container save integration

This note describes the integration that is currently implemented. Save format
version 2 adds mob snapshots and generic container records while retaining read
compatibility with version 1 files.

## Runtime boundary

`App::new` loads `SaveData`, rebuilds the live `MobManager` from `data.mobs`, and
rebuilds its furnace map from valid furnace container records.

Before a normal save or a gauntlet save round-trip, `capture_live_state`:

1. snapshots live, nearby mobs with `SaveData::capture_mobs`;
2. preserves container kinds this build does not understand;
3. replaces all furnace records with snapshots of the live furnace map.

This keeps `SaveData` as the disk representation and the mob manager and furnace
map as the live representation. Unknown future container kinds survive a
load/save cycle.

## Mob records

`MobSave` stores kind, position, velocity, yaw, health, and grounded state. It
deliberately omits entity IDs and temporary AI state. Dead mobs and mobs beyond
`MOB_SAVE_RADIUS` are not written.

Mob kind numbers are encoded explicitly in `save.rs`; enum declaration order is
not an on-disk contract.

## Furnace records

`ContainerSave` is generic: a stable kind number, a vector of item slots, and a
vector of numeric fields. A furnace uses three slots (input, fuel, output) and
three fields (burn remaining, original burn duration, smelting progress).

`ContainerSave::from_furnace` and `ContainerSave::to_furnace` are the only
conversion boundary. The decoder accepts only the furnace kind, exact slot and
field counts, and finite nonnegative timing values.

## Compatibility and validation

- Version 1 files load with empty mob and container sections.
- Version 2 readers reject corrupt counts and invalid mob values.
- Unknown container kinds remain opaque records so an older build does not
  destroy newer data.
- Save tests cover mob and container byte round-trips, version 1 compatibility,
  malformed input, live-state capture, and a mid-smelt furnace conversion.

Any new runtime container type should follow the furnace pattern: define a
stable `ContainerKind`, validate its exact shape in one conversion function,
restore it in `App::new`, and snapshot it in `capture_live_state`.
