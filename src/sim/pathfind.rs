//! A* over the voxel grid for a one-block-wide, two-block-tall walker.
//!
//! The walker can step up one block, drop up to three, and refuses solid cells and
//! unsupported edges. Crucially it reads *partially carved* blocks through
//! `sub_solid` / `fill_ratio`, so a tunnel the player bored by hand becomes a mob
//! highway -- which is the whole reason quiet mining matters.
//!
//! The search is hard capped on expanded nodes and returns the best partial path when
//! it hits the cap, so it can never run unboundedly. Mobs recompute on a timer, never
//! every frame.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use glam::IVec3;

use crate::sim::sound::VoxelWorld;

// =============================================================================
// ============================  TUNING BLOCK  =================================
// =============================================================================

/// Walker footprint: one block wide, this many blocks tall.
pub const AGENT_HEIGHT: i32 = 2;
/// Tallest lip the walker climbs without a path break.
pub const MAX_STEP_UP: i32 = 1;
/// Furthest the walker will voluntarily drop.
pub const MAX_DROP: i32 = 3;

/// A block this empty or emptier is walked straight through, whatever shape the
/// remaining sub-voxels are. Your half-mined wall stops being a wall here.
pub const PASSABLE_FILL: f32 = 0.35;
/// A block this full or fuller holds the walker's weight.
pub const SUPPORT_FILL: f32 = 0.15;

/// Extra cost of climbing a block, relative to a flat step of 1.0.
pub const STEP_UP_COST: f32 = 0.6;
/// Extra cost per block of falling.
pub const DROP_COST: f32 = 0.2;
/// >1 makes the search greedier: fewer nodes, slightly worse paths. Good trade here.
pub const HEURISTIC_WEIGHT: f32 = 1.25;

/// Hard cap on expanded nodes for a normal mob repath.
pub const MAX_EXPANDED_NODES: usize = 1200;
/// Vertical span the goal snapper will search to find standable ground.
pub const GOAL_SNAP_RANGE: i32 = 4;

pub const WORLD_MIN_Y: i32 = 0;
pub const WORLD_MAX_Y: i32 = 256;

// =============================================================================
// ==========================  END TUNING BLOCK  ===============================
// =============================================================================

/// True when the walker's body cannot occupy this block.
///
/// Air is free. Untouched solid rock blocks. A partially carved block is passable
/// when it is mostly gone, or when its centre core has been hollowed out -- which is
/// exactly the shape a bored tunnel leaves behind.
pub fn cell_blocked<W: VoxelWorld + ?Sized>(world: &W, p: IVec3) -> bool {
    let b = world.block_at(p.x, p.y, p.z);
    if b.is_air() || !b.is_solid() {
        return false;
    }
    let fill = world.fill_ratio(p.x, p.y, p.z);
    if fill >= 1.0 {
        return true;
    }
    if fill <= PASSABLE_FILL {
        return false;
    }
    !core_clear(world, p)
}

/// Whether the middle 4x4x4 of a block's sub-voxels is empty. That is a hole at
/// least half a block across in every direction, which a walker can use.
fn core_clear<W: VoxelWorld + ?Sized>(world: &W, p: IVec3) -> bool {
    for sy in 2..6 {
        for sz in 2..6 {
            for sx in 2..6 {
                if world.sub_solid(p.x, p.y, p.z, sx, sy, sz) {
                    return false;
                }
            }
        }
    }
    true
}

/// Whether this block can be stood on.
pub fn supports<W: VoxelWorld + ?Sized>(world: &W, p: IVec3) -> bool {
    if p.y < WORLD_MIN_Y {
        return false;
    }
    let b = world.block_at(p.x, p.y, p.z);
    if !b.is_solid() {
        return false;
    }
    let fill = world.fill_ratio(p.x, p.y, p.z);
    if fill >= SUPPORT_FILL {
        return true;
    }
    // Almost gone, but a surviving top skin is still a floor.
    for sz in 2..6 {
        for sx in 2..6 {
            if world.sub_solid(p.x, p.y, p.z, sx, 7, sz) {
                return true;
            }
        }
    }
    false
}

/// Whether the walker's whole body fits at `feet`, ignoring what is underneath.
pub fn body_clear<W: VoxelWorld + ?Sized>(world: &W, feet: IVec3) -> bool {
    if feet.y < WORLD_MIN_Y || feet.y + AGENT_HEIGHT > WORLD_MAX_Y {
        return false;
    }
    for h in 0..AGENT_HEIGHT {
        if cell_blocked(world, feet + IVec3::new(0, h, 0)) {
            return false;
        }
    }
    true
}

/// Whether the walker can stand here: body fits and the block below holds it.
pub fn standable<W: VoxelWorld + ?Sized>(world: &W, feet: IVec3) -> bool {
    body_clear(world, feet) && supports(world, feet - IVec3::Y)
}

/// Snap `p` to the nearest standable cell within `GOAL_SNAP_RANGE` vertically.
///
/// Mobs chase things that are mid-air or standing on a slab; this turns those into
/// a cell the search can actually reach.
pub fn snap_to_ground<W: VoxelWorld + ?Sized>(world: &W, p: IVec3) -> Option<IVec3> {
    if standable(world, p) {
        return Some(p);
    }
    for d in 1..=GOAL_SNAP_RANGE {
        let down = p - IVec3::new(0, d, 0);
        if standable(world, down) {
            return Some(down);
        }
        let up = p + IVec3::new(0, d, 0);
        if standable(world, up) {
            return Some(up);
        }
    }
    None
}

/// The result of one search.
#[derive(Clone, Debug, Default)]
pub struct Path {
    /// Cells from the start to the end of the route, inclusive. Empty only when the
    /// start itself was unusable.
    pub waypoints: Vec<IVec3>,
    /// True when `waypoints` actually ends at the requested goal.
    pub reached_goal: bool,
    /// True when the search stopped because it hit the node cap.
    pub truncated: bool,
    /// Nodes expanded. Always `<= max_nodes`.
    pub expanded: usize,
}

impl Path {
    pub fn is_empty(&self) -> bool {
        self.waypoints.is_empty()
    }
    pub fn len(&self) -> usize {
        self.waypoints.len()
    }
    pub fn end(&self) -> Option<IVec3> {
        self.waypoints.last().copied()
    }
}

/// Heap entry. Ordered by `f`, with a deterministic tiebreak so runs reproduce.
#[derive(Copy, Clone)]
struct Node {
    f: f32,
    g: f32,
    order: u32,
    cell: IVec3,
}
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f && self.order == other.order
    }
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: BinaryHeap is a max-heap, we want the lowest f first.
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.order.cmp(&self.order))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

const HORIZONTAL: [IVec3; 4] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

fn heuristic(from: IVec3, goal: IVec3) -> f32 {
    let dx = (goal.x - from.x).abs() as f32;
    let dz = (goal.z - from.z).abs() as f32;
    let climb = (goal.y - from.y).max(0) as f32;
    HEURISTIC_WEIGHT * (dx + dz + climb * STEP_UP_COST)
}

/// Every move available from `feet`, as `(destination, cost)`.
///
/// Public so mob AI can ask "can I get out of here at all" without a full search.
pub fn successors<W: VoxelWorld + ?Sized>(world: &W, feet: IVec3, out: &mut Vec<(IVec3, f32)>) {
    out.clear();
    // Headroom above the walker's own head, needed before it can climb.
    let can_climb = !cell_blocked(world, feet + IVec3::new(0, AGENT_HEIGHT, 0));

    for dir in HORIZONTAL {
        let side = feet + dir;

        if standable(world, side) {
            out.push((side, 1.0));
            continue;
        }

        if can_climb && MAX_STEP_UP >= 1 {
            let up = side + IVec3::Y;
            if standable(world, up) {
                out.push((up, 1.0 + STEP_UP_COST));
                continue;
            }
        }

        // Nothing at this level or one up: try walking off the edge and falling.
        if !body_clear(world, side) {
            continue;
        }
        for d in 1..=MAX_DROP {
            let down = side - IVec3::new(0, d, 0);
            if standable(world, down) {
                out.push((down, 1.0 + DROP_COST * d as f32));
                break;
            }
            if cell_blocked(world, down) {
                break; // the fall is obstructed
            }
        }
    }
}

/// A* from `start` to `goal` for the 1x2 walker.
///
/// Never expands more than `max_nodes`. If the goal is unreachable within that cap
/// -- or unreachable at all -- the returned path leads to the closest cell the search
/// found, with `reached_goal` false. Callers should walk a partial path anyway: it
/// makes progress, and the next repath continues from further along.
pub fn find_path<W: VoxelWorld + ?Sized>(
    world: &W,
    start: IVec3,
    goal: IVec3,
    max_nodes: usize,
) -> Path {
    let mut path = Path::default();
    if !body_clear(world, start) {
        return path;
    }
    // Aim at a cell the walker could actually occupy.
    let goal = snap_to_ground(world, goal).unwrap_or(goal);

    if start == goal {
        path.waypoints.push(start);
        path.reached_goal = true;
        return path;
    }

    // cell -> (g, parent)
    let mut came: HashMap<IVec3, (f32, IVec3)> = HashMap::new();
    let mut open: BinaryHeap<Node> = BinaryHeap::new();
    came.insert(start, (0.0, start));
    open.push(Node {
        f: heuristic(start, goal),
        g: 0.0,
        order: 0,
        cell: start,
    });

    let mut order = 1u32;
    let mut expanded = 0usize;
    let mut truncated = false;
    // Closest node seen, for the partial-path fallback.
    let mut best = (heuristic(start, goal), 0.0f32, start);
    let mut found = false;
    let mut succ: Vec<(IVec3, f32)> = Vec::with_capacity(8);

    while let Some(node) = open.pop() {
        if came.get(&node.cell).is_some_and(|&(g, _)| node.g > g) {
            continue; // stale entry
        }
        if node.cell == goal {
            found = true;
            best = (0.0, node.g, node.cell);
            break;
        }
        if expanded >= max_nodes {
            truncated = true;
            break;
        }
        expanded += 1;

        successors(world, node.cell, &mut succ);
        for &(next, step) in &succ {
            let ng = node.g + step;
            let improved = match came.get(&next) {
                Some(&(g, _)) => ng < g,
                None => true,
            };
            if !improved {
                continue;
            }
            came.insert(next, (ng, node.cell));
            let h = heuristic(next, goal);
            // Prefer closer to the goal; break ties on cheaper arrival.
            if h < best.0 || (h == best.0 && ng < best.1) {
                best = (h, ng, next);
            }
            open.push(Node {
                f: ng + h,
                g: ng,
                order,
                cell: next,
            });
            order = order.wrapping_add(1);
        }
    }

    let target = if found { goal } else { best.2 };
    path.waypoints = reconstruct(&came, start, target);
    path.reached_goal = found;
    path.truncated = truncated;
    path.expanded = expanded;
    path
}

fn reconstruct(came: &HashMap<IVec3, (f32, IVec3)>, start: IVec3, end: IVec3) -> Vec<IVec3> {
    let mut out = Vec::new();
    let mut cur = end;
    // The guard bounds a corrupted-parent cycle; it can only trip on a bug.
    for _ in 0..came.len() + 1 {
        out.push(cur);
        if cur == start {
            break;
        }
        match came.get(&cur) {
            Some(&(_, parent)) => cur = parent,
            None => break,
        }
    }
    out.reverse();
    if out.first() != Some(&start) {
        out.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::block::BlockId;
    use crate::sim::sound::mock::MockWorld;

    const GROUND: i32 = 64;

    fn flat() -> MockWorld {
        MockWorld::flat_ground(48, GROUND)
    }

    fn feet(x: i32, z: i32) -> IVec3 {
        IVec3::new(x, GROUND + 1, z)
    }

    #[test]
    fn walks_a_straight_line_across_flat_ground() {
        let w = flat();
        let p = find_path(&w, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert!(p.reached_goal, "{p:?}");
        assert_eq!(p.waypoints.first(), Some(&feet(0, 0)));
        assert_eq!(p.waypoints.last(), Some(&feet(8, 0)));
        assert_eq!(p.waypoints.len(), 9, "should be the direct 8-step route");
    }

    #[test]
    fn routes_around_a_wall() {
        let mut w = flat();
        // A wall across z = -4..4 at x = 4, with a gap at z = 5.
        w.fill((4, GROUND + 1, -4), (4, GROUND + 4, 4), BlockId::STONE);

        let p = find_path(&w, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert!(p.reached_goal, "should find the way around: {p:?}");
        // It must not tunnel through the wall.
        for wp in &p.waypoints {
            assert!(
                !(wp.x == 4 && (-4..=4).contains(&wp.z) && wp.y <= GROUND + 4),
                "path went through the wall at {wp:?}"
            );
        }
        assert!(
            p.waypoints.len() > 9,
            "the detour must be longer than the direct route"
        );
    }

    #[test]
    fn refuses_an_impossible_path() {
        let mut w = flat();
        // Seal the goal inside a stone box.
        w.fill((7, GROUND + 1, -1), (9, GROUND + 4, 1), BlockId::STONE);
        w.set(8, GROUND + 1, 0, BlockId::AIR);
        w.set(8, GROUND + 2, 0, BlockId::AIR);

        let p = find_path(&w, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert!(!p.reached_goal, "a sealed goal must not be reached: {p:?}");
        assert_ne!(p.end(), Some(feet(8, 0)));
    }

    #[test]
    fn a_fully_enclosed_start_goes_nowhere() {
        let mut w = MockWorld::solid_stone();
        w.set(0, GROUND + 1, 0, BlockId::AIR);
        w.set(0, GROUND + 2, 0, BlockId::AIR);

        let p = find_path(&w, feet(0, 0), feet(20, 0), MAX_EXPANDED_NODES);
        assert!(!p.reached_goal);
        assert_eq!(p.waypoints, vec![feet(0, 0)], "nowhere to go but stay put");
        assert!(
            !p.truncated,
            "an enclosed start exhausts, it does not hit the cap"
        );
    }

    #[test]
    fn terminates_at_the_node_cap_and_returns_a_useful_partial() {
        let w = flat();
        // Deliberately tiny cap over a long open route.
        let cap = 20;
        let p = find_path(&w, feet(0, 0), feet(40, 0), cap);
        assert!(p.truncated, "should have hit the cap: {p:?}");
        assert!(!p.reached_goal);
        assert!(
            p.expanded <= cap,
            "expanded {} exceeds cap {}",
            p.expanded,
            cap
        );
        assert!(
            p.waypoints.len() > 1,
            "a partial path must still make progress"
        );
        let end = p.end().unwrap();
        let start_d = (feet(40, 0) - feet(0, 0)).abs().element_sum();
        let end_d = (feet(40, 0) - end).abs().element_sum();
        assert!(
            end_d < start_d,
            "the partial path must get closer to the goal"
        );
    }

    #[test]
    fn steps_up_one_block_but_not_two() {
        let mut w = flat();
        w.fill((4, GROUND + 1, -48), (4, GROUND + 1, 48), BlockId::STONE); // 1-high lip
        let p = find_path(
            &w,
            feet(0, 0),
            IVec3::new(8, GROUND + 1, 0),
            MAX_EXPANDED_NODES,
        );
        assert!(p.reached_goal, "a one-block lip is walkable: {p:?}");
        assert!(
            p.waypoints.iter().any(|c| c.y == GROUND + 2),
            "the route should climb onto the lip"
        );

        let mut w2 = flat();
        w2.fill((4, GROUND + 1, -48), (4, GROUND + 2, 48), BlockId::STONE); // 2-high wall
        let p2 = find_path(
            &w2,
            feet(0, 0),
            IVec3::new(8, GROUND + 1, 0),
            MAX_EXPANDED_NODES,
        );
        assert!(
            !p2.reached_goal,
            "a two-block wall with no way round must fail: {p2:?}"
        );
    }

    #[test]
    fn drops_three_blocks_but_refuses_five() {
        // A ledge at GROUND, with the floor beyond it three blocks lower.
        let mut w = MockWorld::air();
        w.fill((-8, GROUND - 2, -8), (0, GROUND, 8), BlockId::STONE);
        w.fill((1, GROUND - 5, -8), (12, GROUND - 3, 8), BlockId::STONE);
        let p = find_path(
            &w,
            feet(0, 0),
            IVec3::new(6, GROUND - 2, 0),
            MAX_EXPANDED_NODES,
        );
        assert!(p.reached_goal, "a three-block drop is allowed: {p:?}");

        let mut deep = MockWorld::air();
        deep.fill((-8, GROUND - 2, -8), (0, GROUND, 8), BlockId::STONE);
        deep.fill((1, GROUND - 7, -8), (12, GROUND - 5, 8), BlockId::STONE);
        let p2 = find_path(
            &deep,
            feet(0, 0),
            IVec3::new(6, GROUND - 4, 0),
            MAX_EXPANDED_NODES,
        );
        assert!(
            !p2.reached_goal,
            "a five-block drop must be refused: {p2:?}"
        );
    }

    #[test]
    fn will_not_walk_out_over_thin_air() {
        // An island with nothing around it.
        let mut w = MockWorld::air();
        w.fill((-2, GROUND, -2), (2, GROUND, 2), BlockId::STONE);
        let p = find_path(&w, feet(0, 0), feet(10, 0), MAX_EXPANDED_NODES);
        assert!(!p.reached_goal);
        for wp in &p.waypoints {
            assert!(
                wp.x.abs() <= 2 && wp.z.abs() <= 2,
                "walked off the island to {wp:?}"
            );
        }
    }

    #[test]
    fn a_carved_tunnel_is_a_mob_highway() {
        // Solid stone with a 1x2 tunnel bored through it by hand.
        let mut sealed = MockWorld::solid_stone();
        sealed.fill((-1, GROUND + 1, 0), (0, GROUND + 2, 0), BlockId::AIR);
        sealed.fill((10, GROUND + 1, 0), (12, GROUND + 2, 0), BlockId::AIR);

        let blocked = find_path(&sealed, feet(0, 0), feet(11, 0), MAX_EXPANDED_NODES);
        assert!(!blocked.reached_goal, "solid rock is not a path");

        let mut bored = sealed.clone();
        bored.fill((1, GROUND + 1, 0), (9, GROUND + 2, 0), BlockId::AIR);
        let open = find_path(&bored, feet(0, 0), feet(11, 0), MAX_EXPANDED_NODES);
        assert!(open.reached_goal, "the tunnel must be walkable: {open:?}");
    }

    #[test]
    fn a_mostly_hollow_block_is_passable() {
        let mut w = flat();
        // Two blocks of stone in the way...
        w.fill((4, GROUND + 1, -48), (4, GROUND + 2, 48), BlockId::STONE);
        let before = find_path(&w, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert!(!before.reached_goal, "untouched stone blocks the way");

        // ...chewed down to a fraction of themselves at z = 0 only.
        let mut chewed = w.clone();
        for y in GROUND + 1..=GROUND + 2 {
            for sy in 0..8 {
                for sz in 0..8 {
                    for sx in 0..8 {
                        // leave a sparse skeleton: under PASSABLE_FILL
                        if (sx + sy + sz) % 5 != 0 {
                            chewed.carve(4, y, 0, sx, sy, sz);
                        }
                    }
                }
            }
        }
        assert!(chewed.fill_ratio(4, GROUND + 1, 0) < PASSABLE_FILL);
        let after = find_path(&chewed, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert!(
            after.reached_goal,
            "a mostly-hollow block must be passable: {after:?}"
        );
        assert!(
            after.waypoints.contains(&feet(4, 0)),
            "it should go straight through"
        );
    }

    #[test]
    fn a_bored_core_is_passable_even_when_the_block_is_mostly_full() {
        let mut w = flat();
        w.fill((4, GROUND + 1, -48), (4, GROUND + 2, 48), BlockId::STONE);
        let mut bored = w.clone();
        // Punch out only the central 4x4x4 core -- 64/512 = 12.5% removed, so
        // fill_ratio stays high and only sub_solid can tell this is passable.
        for y in GROUND + 1..=GROUND + 2 {
            for sy in 2..6 {
                for sz in 2..6 {
                    for sx in 2..6 {
                        bored.carve(4, y, 0, sx, sy, sz);
                    }
                }
            }
        }
        assert!(
            bored.fill_ratio(4, GROUND + 1, 0) > PASSABLE_FILL,
            "still mostly solid"
        );
        let p = find_path(&bored, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert!(p.reached_goal, "a bored core must be passable: {p:?}");
        assert!(p.waypoints.contains(&feet(4, 0)));
    }

    #[test]
    fn snaps_a_mid_air_goal_down_to_the_floor() {
        let w = flat();
        let airborne = IVec3::new(6, GROUND + 4, 0);
        assert_eq!(snap_to_ground(&w, airborne), Some(feet(6, 0)));
        let p = find_path(&w, feet(0, 0), airborne, MAX_EXPANDED_NODES);
        assert!(p.reached_goal);
        assert_eq!(p.end(), Some(feet(6, 0)));
    }

    #[test]
    fn results_are_deterministic() {
        let mut w = flat();
        w.fill((4, GROUND + 1, -4), (4, GROUND + 4, 4), BlockId::STONE);
        let a = find_path(&w, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        let b = find_path(&w, feet(0, 0), feet(8, 0), MAX_EXPANDED_NODES);
        assert_eq!(a.waypoints, b.waypoints);
    }
}
