//! Split panes: a tab shows several sessions side by side or stacked (DESIGN §8.9).
//!
//! Each pane is a session of its own, so the queue, the web app and restore see it as
//! one. The daemon keeps one `Pane` tree per tab with more than one pane, so every
//! client draws the same splits; a tab of one session has no tree.

use crate::SessionId;
use serde::{Deserialize, Serialize};

/// The whole of a split: how much `a` gets, in thousandths.
pub const RATIO_SCALE: u16 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Pane {
    Leaf {
        session: SessionId,
    },
    Split {
        axis: Axis,
        /// `a`'s share of the room left after the divider, in `RATIO_SCALE`ths.
        ratio: u16,
        a: Box<Pane>,
        b: Box<Pane>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Side by side: `a` on the left, `b` on the right, a `│` between.
    Columns,
    /// Stacked: `a` above, `b` below, a `─` between.
    Rows,
}

/// Where a new pane goes, beside the one it splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Up,
    Down,
    Left,
    Right,
}

/// A rectangle of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Area {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

impl Area {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x + self.cols && y >= self.y && y < self.y + self.rows
    }
}

/// The line between two halves of a split, one cell thick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divider {
    pub axis: Axis,
    /// The divider's own cells: one column (`Columns`) or one row (`Rows`).
    pub area: Area,
    /// A session on each side; the split is the one where they part (`set_ratio`).
    pub a: SessionId,
    pub b: SessionId,
    /// Where the split's room starts and how long it is, along the axis, to turn a
    /// pointer position into a ratio.
    pub start: u16,
    pub room: u16,
}

impl Divider {
    /// The ratio that puts this divider at `pos` (a column or row of the terminal).
    pub fn ratio_at(&self, pos: u16) -> u16 {
        // Rounded up, so `layout` puts the divider on `pos`, not a cell short of it.
        let share = pos.saturating_sub(self.start) as u32 * RATIO_SCALE as u32;
        share
            .div_ceil(self.room.max(1) as u32)
            .min(RATIO_SCALE as u32) as u16
    }
}

impl Pane {
    pub fn leaf(session: SessionId) -> Pane {
        Pane::Leaf { session }
    }

    /// Its sessions, left to right and top to bottom.
    pub fn sessions(&self) -> Vec<SessionId> {
        let mut out = Vec::new();
        self.walk(&mut out);
        out
    }

    fn walk(&self, out: &mut Vec<SessionId>) {
        match self {
            Pane::Leaf { session } => out.push(*session),
            Pane::Split { a, b, .. } => {
                a.walk(out);
                b.walk(out);
            }
        }
    }

    pub fn first(&self) -> SessionId {
        match self {
            Pane::Leaf { session } => *session,
            Pane::Split { a, .. } => a.first(),
        }
    }

    pub fn contains(&self, id: SessionId) -> bool {
        match self {
            Pane::Leaf { session } => *session == id,
            Pane::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Puts `new` on `side` of `at`, halving `at`'s room. False if `at` isn't here.
    pub fn split(&mut self, at: SessionId, new: SessionId, side: Side) -> bool {
        match self {
            Pane::Leaf { session } if *session == at => {
                let (old, new) = (Box::new(Pane::leaf(at)), Box::new(Pane::leaf(new)));
                let axis = match side {
                    Side::Left | Side::Right => Axis::Columns,
                    Side::Up | Side::Down => Axis::Rows,
                };
                let (a, b) = match side {
                    Side::Right | Side::Down => (old, new),
                    Side::Left | Side::Up => (new, old),
                };
                *self = Pane::Split {
                    axis,
                    ratio: RATIO_SCALE / 2,
                    a,
                    b,
                };
                true
            }
            Pane::Leaf { .. } => false,
            Pane::Split { a, b, .. } => a.split(at, new, side) || b.split(at, new, side),
        }
    }

    /// The tree without the sessions `keep` rejects; a split left with one side
    /// becomes that side. `None` when nothing is kept.
    pub fn retain(self, keep: &impl Fn(SessionId) -> bool) -> Option<Pane> {
        match self {
            Pane::Leaf { session } => keep(session).then_some(self),
            Pane::Split { axis, ratio, a, b } => match (a.retain(keep), b.retain(keep)) {
                (Some(a), Some(b)) => Some(Pane::Split {
                    axis,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, other) => one.or(other),
            },
        }
    }

    /// Sets the ratio of the split where `a` and `b` part. False if there is none.
    pub fn set_ratio(&mut self, a_id: SessionId, b_id: SessionId, to: u16) -> bool {
        let Pane::Split { ratio, a, b, .. } = self else {
            return false;
        };
        if a.contains(a_id) && b.contains(b_id) {
            *ratio = to.clamp(1, RATIO_SCALE - 1);
            true
        } else if a.contains(a_id) && a.contains(b_id) {
            a.set_ratio(a_id, b_id, to)
        } else if b.contains(a_id) && b.contains(b_id) {
            b.set_ratio(a_id, b_id, to)
        } else {
            false
        }
    }

    /// Where each pane goes in `area`, and the dividers between them.
    pub fn layout(&self, area: Area) -> (Vec<(SessionId, Area)>, Vec<Divider>) {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        self.place(area, &mut panes, &mut dividers);
        (panes, dividers)
    }

    fn place(&self, area: Area, panes: &mut Vec<(SessionId, Area)>, dividers: &mut Vec<Divider>) {
        let Pane::Split { axis, ratio, a, b } = self else {
            panes.push((self.first(), area));
            return;
        };
        let (start, len) = match axis {
            Axis::Columns => (area.x, area.cols),
            Axis::Rows => (area.y, area.rows),
        };
        // One cell for the divider; each side keeps at least one when it can.
        let room = len.saturating_sub(1);
        let first = ((room as u32 * *ratio as u32) / RATIO_SCALE as u32) as u16;
        let first = if room >= 2 {
            first.clamp(1, room - 1)
        } else {
            room
        };
        let second = room - first;
        let (a_area, line, b_area) = match axis {
            Axis::Columns => (
                Area {
                    cols: first,
                    ..area
                },
                Area {
                    x: area.x + first,
                    cols: len.min(1),
                    ..area
                },
                Area {
                    x: area.x + first + 1,
                    cols: second,
                    ..area
                },
            ),
            Axis::Rows => (
                Area {
                    rows: first,
                    ..area
                },
                Area {
                    y: area.y + first,
                    rows: len.min(1),
                    ..area
                },
                Area {
                    y: area.y + first + 1,
                    rows: second,
                    ..area
                },
            ),
        };
        a.place(a_area, panes, dividers);
        dividers.push(Divider {
            axis: *axis,
            area: line,
            a: a.first(),
            b: b.first(),
            start,
            room,
        });
        b.place(b_area, panes, dividers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(cols: u16, rows: u16) -> Area {
        Area {
            x: 0,
            y: 0,
            cols,
            rows,
        }
    }

    #[test]
    fn splitting_puts_the_new_pane_on_the_asked_side() {
        let mut p = Pane::leaf(1);
        assert!(p.split(1, 2, Side::Right));
        assert!(p.split(1, 3, Side::Up));
        assert!(!p.split(9, 4, Side::Down));
        assert_eq!(p.sessions(), vec![3, 1, 2]);
        let (panes, dividers) = p.layout(area(81, 25));
        assert_eq!(
            panes,
            vec![
                (
                    3,
                    Area {
                        x: 0,
                        y: 0,
                        cols: 40,
                        rows: 12
                    }
                ),
                (
                    1,
                    Area {
                        x: 0,
                        y: 13,
                        cols: 40,
                        rows: 12
                    }
                ),
                (
                    2,
                    Area {
                        x: 41,
                        y: 0,
                        cols: 40,
                        rows: 25
                    }
                ),
            ]
        );
        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].axis, Axis::Rows);
        assert_eq!(dividers[0].area.y, 12);
        assert_eq!((dividers[1].a, dividers[1].b), (3, 2));
        assert_eq!(dividers[1].area.x, 40);
    }

    #[test]
    fn removing_a_pane_gives_its_room_to_its_sibling() {
        let mut p = Pane::leaf(1);
        p.split(1, 2, Side::Right);
        p.split(2, 3, Side::Down);
        let p = p.retain(&|id| id != 2).unwrap();
        assert_eq!(p.sessions(), vec![1, 3]);
        let p = p.retain(&|id| id != 1).unwrap();
        assert_eq!(p, Pane::leaf(3));
        assert_eq!(p.retain(&|_| false), None);
    }

    #[test]
    fn ratio_is_set_where_two_panes_part() {
        let mut p = Pane::leaf(1);
        p.split(1, 2, Side::Right);
        p.split(2, 3, Side::Down);
        assert!(p.set_ratio(2, 3, 250));
        assert!(p.set_ratio(1, 3, 700));
        assert!(!p.set_ratio(1, 9, 100));
        let Pane::Split { ratio, b, .. } = &p else {
            panic!()
        };
        assert_eq!(*ratio, 700);
        let Pane::Split { ratio, .. } = &**b else {
            panic!()
        };
        assert_eq!(*ratio, 250);
    }

    #[test]
    fn tiny_areas_never_underflow() {
        let mut p = Pane::leaf(1);
        p.split(1, 2, Side::Right);
        p.split(2, 3, Side::Down);
        for cols in 0..4 {
            for rows in 0..4 {
                let (panes, _) = p.layout(area(cols, rows));
                assert_eq!(panes.len(), 3);
                for (_, a) in panes {
                    assert!(a.x + a.cols <= cols.max(1) && a.y + a.rows <= rows.max(1));
                }
            }
        }
    }

    #[test]
    fn a_pointer_position_gives_a_ratio() {
        let mut p = Pane::leaf(1);
        p.split(1, 2, Side::Right);
        let d = p.layout(area(81, 24)).1[0];
        assert_eq!(d.ratio_at(d.start + d.room / 4), 250);
        // The divider lands where the pointer let go, whatever the room.
        for cols in [20, 81, 100, 233] {
            let mut p = p.clone();
            let d = p.layout(area(cols, 5)).1[0];
            for pos in 1..cols - 1 {
                p.set_ratio(1, 2, d.ratio_at(pos));
                assert_eq!(p.layout(area(cols, 5)).1[0].area.x, pos, "{cols} cols");
            }
        }
    }
}
