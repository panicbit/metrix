//! Instruments that track values and/or derive values
//! from observations.
use std::time::Instant;

use crate::{Observation, ObservedValue, PutsSnapshot, TimeUnit};

pub use self::counter::Counter;
pub use self::gauge::*;
pub use self::histogram::Histogram;
pub use self::instrument_adapter::*;
pub use self::meter::Meter;
pub use self::other_instruments::*;
pub use self::panel::*;
pub use self::polled::*;
pub use self::switches::*;
pub use crate::cockpit::Cockpit;

mod counter;
mod gauge;
mod histogram;
mod instrument_adapter;
mod meter;
pub mod other_instruments;
mod panel;
pub mod polled;
pub mod switches;

#[derive(Debug, Clone)]
/// An update instruction for an instrument
pub enum Update {
    /// Many observations without a value observed at a given time
    Observations(u64, Instant),
    /// One observation without a value observed at a given time
    Observation(Instant),
    /// One observation with a value observed at a given time
    ObservationWithValue(ObservedValue, Instant),
}

/// A label with the associated `Update`
///
/// This is basically a split `Observation`
pub struct LabelAndUpdate<T>(pub T, pub Update);

impl<T> From<Observation<T>> for LabelAndUpdate<T> {
    fn from(obs: Observation<T>) -> LabelAndUpdate<T> {
        match obs {
            Observation::Observed {
                label,
                count,
                timestamp,
                ..
            } => LabelAndUpdate(label, Update::Observations(count, timestamp)),
            Observation::ObservedOne {
                label, timestamp, ..
            } => LabelAndUpdate(label, Update::Observation(timestamp)),
            Observation::ObservedOneValue {
                label,
                value,
                timestamp,
                ..
            } => LabelAndUpdate(label, Update::ObservationWithValue(value, timestamp)),
        }
    }
}

/// A label with the associated `Update`
///
/// This is basically a split `Observation`
pub struct BorrowedLabelAndUpdate<'a, T: 'a>(pub &'a T, pub Update);

impl<'a, T> From<&'a Observation<T>> for BorrowedLabelAndUpdate<'a, T> {
    fn from(obs: &'a Observation<T>) -> BorrowedLabelAndUpdate<'a, T> {
        match obs {
            Observation::Observed {
                label,
                count,
                timestamp,
                ..
            } => BorrowedLabelAndUpdate(label, Update::Observations(*count, *timestamp)),
            Observation::ObservedOne {
                label, timestamp, ..
            } => BorrowedLabelAndUpdate(label, Update::Observation(*timestamp)),
            Observation::ObservedOneValue {
                label,
                value,
                timestamp,
                ..
            } => BorrowedLabelAndUpdate(label, Update::ObservationWithValue(*value, *timestamp)),
        }
    }
}

/// Implementors of `Updates`
/// can handle `Update`s.
///
/// `Update`s are basically observations without a label.
pub trait Updates {
    /// Update the internal state according to the given `Update`.
    ///
    /// Not all `Update`s might modify the internal state.
    /// Only those that are appropriate and meaningful for
    /// the implementor.
    ///
    /// Returns the number of instruments updated
    fn update(&mut self, with: &Update) -> usize;
}

/// Requirement for an instrument
pub trait Instrument: Updates + PutsSnapshot {}

pub(crate) enum LabelFilter<L> {
    AcceptNone,
    AcceptAll,
    One(L),
    Two(L, L),
    Three(L, L, L),
    Four(L, L, L, L),
    Five(L, L, L, L, L),
    Many(Vec<L>),
}

impl<L> LabelFilter<L>
where
    L: PartialEq + Eq,
{
    pub fn new(label: L) -> Self {
        Self::One(label)
    }

    pub fn many(mut labels: Vec<L>) -> Self {
        if labels.is_empty() {
            return LabelFilter::AcceptNone;
        }

        if labels.len() == 1 {
            return LabelFilter::One(labels.pop().unwrap());
        }

        if labels.len() == 2 {
            let a = labels.pop().unwrap();
            let b = labels.pop().unwrap();
            return LabelFilter::Two(b, a);
        }

        if labels.len() == 3 {
            let a = labels.pop().unwrap();
            let b = labels.pop().unwrap();
            let c = labels.pop().unwrap();
            return LabelFilter::Three(c, b, a);
        }

        if labels.len() == 4 {
            let a = labels.pop().unwrap();
            let b = labels.pop().unwrap();
            let c = labels.pop().unwrap();
            let d = labels.pop().unwrap();
            return LabelFilter::Four(d, c, b, a);
        }

        if labels.len() == 5 {
            let a = labels.pop().unwrap();
            let b = labels.pop().unwrap();
            let c = labels.pop().unwrap();
            let d = labels.pop().unwrap();
            let ee = labels.pop().unwrap();
            return LabelFilter::Five(ee, d, c, b, a);
        }

        LabelFilter::Many(labels)
    }

    pub fn accepts(&self, label: &L) -> bool {
        match self {
            LabelFilter::AcceptNone => false,
            LabelFilter::AcceptAll => true,
            LabelFilter::One(a) => label == a,
            LabelFilter::Two(a, b) => label == a || label == b,
            LabelFilter::Three(a, b, c) => label == a || label == b || label == c,
            LabelFilter::Four(a, b, c, d) => label == a || label == b || label == c || label == d,
            LabelFilter::Five(a, b, c, d, ee) => {
                label == a || label == b || label == c || label == d || label == ee
            }
            LabelFilter::Many(many) => many.contains(label),
        }
    }

    pub fn add_label(&mut self, label: L) {
        let current = std::mem::replace(self, LabelFilter::AcceptNone);
        *self = match current {
            LabelFilter::AcceptAll => LabelFilter::AcceptAll,
            LabelFilter::AcceptNone => LabelFilter::One(label),
            LabelFilter::One(a) => LabelFilter::Two(a, label),
            LabelFilter::Two(a, b) => LabelFilter::Three(a, b, label),
            LabelFilter::Three(a, b, c) => LabelFilter::Four(a, b, c, label),
            LabelFilter::Four(a, b, c, d) => LabelFilter::Five(a, b, c, d, label),
            LabelFilter::Five(a, b, c, d, ee) => {
                let mut labels = vec![a, b, c, d, ee];
                labels.push(label);
                LabelFilter::Many(labels)
            }
            LabelFilter::Many(mut labels) => {
                labels.push(label);
                LabelFilter::Many(labels)
            }
        };
    }
}

impl<L> Default for LabelFilter<L> {
    fn default() -> Self {
        Self::AcceptAll
    }
}

fn duration_to_display_value(time: u64, current_unit: TimeUnit, target_unit: TimeUnit) -> u64 {
    use TimeUnit::*;
    match (current_unit, target_unit) {
        (Nanoseconds, Nanoseconds) => time,
        (Nanoseconds, Microseconds) => time / 1_000,
        (Nanoseconds, Milliseconds) => time / 1_000_000,
        (Nanoseconds, Seconds) => time / 1_000_000_000,
        (Microseconds, Nanoseconds) => time * 1_000,
        (Microseconds, Microseconds) => time,
        (Microseconds, Milliseconds) => time / 1_000,
        (Microseconds, Seconds) => time / 1_000_000,
        (Milliseconds, Nanoseconds) => time * 1_000_000,
        (Milliseconds, Microseconds) => time * 1_000,
        (Milliseconds, Milliseconds) => time,
        (Milliseconds, Seconds) => time / 1_000,
        (Seconds, Nanoseconds) => time * 1_000_000_000,
        (Seconds, Microseconds) => time * 1_000_000,
        (Seconds, Milliseconds) => time * 1_000,
        (Seconds, Seconds) => time,
    }
}
