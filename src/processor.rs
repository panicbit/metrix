//! Transmitting observations and grouping metrics.
use std::time::{Duration, Instant};

use crossbeam_channel::{self as channel, Receiver, TryRecvError};

use crate::cockpit::Cockpit;
use crate::instruments::Panel;
use crate::snapshot::{ItemKind, Snapshot};
use crate::util;
use crate::Descriptive;
use crate::{
    HandlesObservations, Observation, ObservationLike, PutsSnapshot, TelemetryTransmitter,
};

/// Implementors can group everything that can process
/// `TelemetryMessage`s.
///
/// Since `PutsSnapshot` implementors can be added almost everywhere
/// the `add_snapshooter` method is placed here, too.
pub trait AggregatesProcessors {
    /// Add a processor.
    fn add_processor<P: ProcessesTelemetryMessages>(&mut self, processor: P);
    /// Add a snapshooter.
    fn add_snapshooter<S: PutsSnapshot>(&mut self, snapshooter: S);
}

/// A message that can be handled by a `ReceivesTelemetryData`
pub(crate) enum TelemetryMessage<L> {
    /// An observation has been made
    Observation(Observation<L>),
    /// A `Cockpit` should be added
    AddCockpit(Cockpit<L>),
    /// An arbitrary `HandlesObservations` should be added
    AddHandler(Box<dyn HandlesObservations<Label = L>>),
    /// Adds a panel to a cockpit with the given name
    ///
    /// This means the cockpit must have a name set.
    AddPanel {
        cockpit_name: String,
        panel: Panel<L>,
    },
}

/// The result of processing
/// messages.
///
/// Used for making decisions for further processing
/// within the `TelemetryDriver`
pub struct ProcessingOutcome {
    pub processed: usize,
    pub dropped: usize,
    pub instruments_updated: usize,
}

impl ProcessingOutcome {
    /// Simply add the corresponding elements
    pub fn combine_with(&mut self, other: &ProcessingOutcome) {
        self.processed += other.processed;
        self.dropped += other.dropped;
        self.instruments_updated += other.instruments_updated;
    }

    pub fn something_happened(&self) -> bool {
        self.processed > 0 || self.dropped > 0
    }
}

impl Default for ProcessingOutcome {
    fn default() -> ProcessingOutcome {
        ProcessingOutcome {
            processed: 0,
            dropped: 0,
            instruments_updated: 0,
        }
    }
}

/// A strategy for processing observations
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ProcessingStrategy {
    /// Process all observations
    ProcessAll,
    /// Drop all observations
    DropAll,
    /// Process only observations that are not older
    /// than the given `Durations` by the time
    /// messages are processed.
    DropOlderThan(Duration),
}

impl ProcessingStrategy {
    pub(crate) fn decider(&self) -> ProcessingDecider {
        match *self {
            ProcessingStrategy::ProcessAll => ProcessingDecider::ProcessAll,
            ProcessingStrategy::DropAll => ProcessingDecider::DropAll,
            ProcessingStrategy::DropOlderThan(max_age) => {
                ProcessingDecider::DropBeforeDeadline(Instant::now() - max_age)
            }
        }
    }
}

impl Default for ProcessingStrategy {
    fn default() -> Self {
        ProcessingStrategy::DropOlderThan(Duration::from_secs(60))
    }
}

pub enum ProcessingDecider {
    ProcessAll,
    DropAll,
    DropBeforeDeadline(Instant),
}

impl ProcessingDecider {
    pub fn should_be_processed<T: ObservationLike>(&self, observation: &T) -> bool {
        match self {
            ProcessingDecider::ProcessAll => true,
            ProcessingDecider::DropAll => false,
            ProcessingDecider::DropBeforeDeadline(drop_deadline) => {
                observation.timestamp() > *drop_deadline
            }
        }
    }
}

/// Can process `TelemetryMessage`.
///
/// This is the counterpart of `TransmitsTelemetryData`.
///
/// Since this mostly results in metrics this
/// trait also requires the capability to write `Snapshot`s.
pub trait ProcessesTelemetryMessages: PutsSnapshot + Send + 'static {
    /// Receive and handle pending operations
    fn process(&mut self, max: usize, strategy: ProcessingStrategy) -> ProcessingOutcome;
}

/// The counterpart of the `TelemetryTransmitter`. It receives the
/// `Observation`s and other messages and processes them.
///
/// A `TelemetryProcessor` is tied to a specific kind of label
/// which is used to determine which metrics are triggered.
///
/// The `TelemetryProcessor<L>` owns a `Receiver`
/// for `TelemetryMessage<L>`.
pub struct TelemetryProcessor<L> {
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    cockpits: Vec<Cockpit<L>>,
    handlers: Vec<Box<dyn HandlesObservations<Label = L>>>,
    receiver: Receiver<TelemetryMessage<L>>,
    snapshooters: Vec<Box<dyn PutsSnapshot>>,
    last_activity_at: Instant,
    max_inactivity_duration: Option<Duration>,
    is_disconnected: bool,
}

impl<L> TelemetryProcessor<L>
where
    L: Clone + Eq + Send + 'static,
{
    /// Creates a `TelemetryTransmitter` and the corresponding
    /// `TelemetryProcessor`
    ///
    /// The `name` will cause a grouping in the `Snapshot`.
    pub fn new_pair<T: Into<String>>(name: T) -> (TelemetryTransmitter<L>, TelemetryProcessor<L>) {
        let (tx, rx) = channel::unbounded();

        let transmitter = TelemetryTransmitter { sender: tx };

        let last_activity_at = Instant::now();
        let max_inactivity_duration = None;

        let receiver = TelemetryProcessor {
            name: Some(name.into()),
            title: None,
            description: None,
            cockpits: Vec::new(),
            handlers: Vec::new(),
            snapshooters: Vec::new(),
            receiver: rx,
            last_activity_at,
            max_inactivity_duration,
            is_disconnected: false,
        };

        (transmitter, receiver)
    }

    /// Creates a `TelemetryTransmitter` and the corresponding
    /// `TelemetryProcessor`
    ///
    /// No grouping will occur unless the name is set.
    pub fn new_pair_without_name() -> (TelemetryTransmitter<L>, TelemetryProcessor<L>) {
        let (tx, rx) = channel::unbounded();

        let transmitter = TelemetryTransmitter { sender: tx };

        let last_activity_at = Instant::now();
        let max_inactivity_duration = None;

        let receiver = TelemetryProcessor {
            name: None,
            title: None,
            description: None,
            cockpits: Vec::new(),
            handlers: Vec::new(),
            snapshooters: Vec::new(),
            receiver: rx,
            last_activity_at,
            max_inactivity_duration,
            is_disconnected: false,
        };

        (transmitter, receiver)
    }

    /// Add a `Cockpit`
    pub fn add_cockpit(&mut self, cockpit: Cockpit<L>) {
        self.cockpits.push(cockpit)
    }

    /// Add a `Cockpit`
    pub fn cockpit(mut self, cockpit: Cockpit<L>) -> Self {
        self.add_cockpit(cockpit);
        self
    }

    /// Returns all contained `Cockpit`s
    #[deprecated(
        since = "0.10.6",
        note = "use get_cockpits. this method will change its signature"
    )]
    pub fn cockpits(&self) -> Vec<&Cockpit<L>> {
        self.get_cockpits()
    }

    /// Returns all contained `Cockpit`s
    pub fn get_cockpits(&self) -> Vec<&Cockpit<L>> {
        self.cockpits.iter().map(|p| p).collect()
    }

    /// Add a (custom) handler for `Observation`s.
    pub fn add_handler<T: HandlesObservations<Label = L>>(&mut self, handler: T) {
        self.handlers.push(Box::new(handler));
    }

    /// Add a (custom) handler for `Observation`s.
    pub fn handler<T: HandlesObservations<Label = L>>(mut self, handler: T) -> Self {
        self.add_handler(handler);
        self
    }

    /// Returns all the handlers
    #[deprecated(
        since = "0.10.6",
        note = "use get_handlers. this method will change its signature"
    )]
    pub fn handlers(&self) -> Vec<&dyn HandlesObservations<Label = L>> {
        self.get_handlers()
    }

    /// Returns all the handlers
    pub fn get_handlers(&self) -> Vec<&dyn HandlesObservations<Label = L>> {
        self.handlers.iter().map(|h| &**h).collect()
    }

    /// Add a snapshooter that simply creates some `Snapshot` defined
    /// by it's internal logic. Usually it polls something when a
    /// `Snapshot` is requested.
    pub fn add_snapshooter<S: PutsSnapshot>(&mut self, snapshooter: S) {
        self.snapshooters.push(Box::new(snapshooter));
    }

    /// Add a snapshooter that simply creates some `Snapshot` defined
    /// by it's internal logic. Usually it polls something when a
    /// `Snapshot` is requested.
    pub fn snapshooter<S: PutsSnapshot>(mut self, snapshooter: S) -> Self {
        self.add_snapshooter(snapshooter);
        self
    }

    #[deprecated(
        since = "0.10.6",
        note = "use get_snapshooters. this method will change its signature"
    )]
    pub fn snapshooters(&self) -> Vec<&dyn PutsSnapshot> {
        self.get_snapshooters()
    }

    pub fn get_snapshooters(&self) -> Vec<&dyn PutsSnapshot> {
        self.snapshooters.iter().map(|p| &**p).collect()
    }

    #[deprecated(
        since = "0.10.6",
        note = "use get_name. this method will change its signature"
    )]
    pub fn name(&self) -> Option<&str> {
        self.get_name()
    }

    pub fn get_name(&self) -> Option<&str> {
        self.name.as_ref().map(|n| &**n)
    }

    /// Sets the name which will cause a grouoing in the `Snapshot`
    pub fn set_name<T: Into<String>>(&mut self, name: T) {
        self.name = Some(name.into())
    }

    /// Sets the maximum amount of time this processor may be
    /// inactive until no more snapshots are taken
    pub fn set_inactivity_limit(&mut self, limit: Duration) {
        self.max_inactivity_duration = Some(limit);
    }

    /// Sets the maximum amount of time this processor may be
    /// inactive until no more snapshots are taken
    pub fn inactivity_limit(mut self, limit: Duration) -> Self {
        self.set_inactivity_limit(limit);
        self
    }

    fn put_values_into_snapshot(&self, into: &mut Snapshot, descriptive: bool) {
        util::put_default_descriptives(self, into, descriptive);

        if let Some(d) = self.max_inactivity_duration {
            if self.last_activity_at.elapsed() > d {
                into.items
                    .push(("_inactive".to_string(), ItemKind::Boolean(true)));
                into.items
                    .push(("_active".to_string(), ItemKind::Boolean(false)));
                return;
            } else {
                into.items
                    .push(("_inactive".to_string(), ItemKind::Boolean(false)));
                into.items
                    .push(("_active".to_string(), ItemKind::Boolean(true)));
            }
        };

        self.cockpits
            .iter()
            .for_each(|c| c.put_snapshot(into, descriptive));

        self.handlers
            .iter()
            .for_each(|h| h.put_snapshot(into, descriptive));

        self.snapshooters
            .iter()
            .for_each(|s| s.put_snapshot(into, descriptive));
    }
}

impl<L> ProcessesTelemetryMessages for TelemetryProcessor<L>
where
    L: Clone + Eq + Send + 'static,
{
    fn process(&mut self, max: usize, strategy: ProcessingStrategy) -> ProcessingOutcome {
        if self.is_disconnected {
            return ProcessingOutcome::default();
        }

        let mut num_received = 0;
        let mut processed = 0;
        let mut instruments_updated = 0;
        let mut dropped = 0;
        let decider = strategy.decider();
        while num_received < max {
            match self.receiver.try_recv() {
                Ok(TelemetryMessage::Observation(obs)) => {
                    if decider.should_be_processed(&obs) {
                        self.cockpits
                            .iter_mut()
                            .for_each(|c| instruments_updated += c.handle_observation(&obs));
                        self.handlers
                            .iter_mut()
                            .for_each(|h| instruments_updated += h.handle_observation(&obs));
                        processed += 1;
                    } else {
                        dropped += 1;
                    }
                }
                Ok(TelemetryMessage::AddCockpit(c)) => {
                    self.add_cockpit(c);
                    processed += 1;
                }
                Ok(TelemetryMessage::AddHandler(h)) => {
                    self.handlers.push(h);
                    processed += 1;
                }
                Ok(TelemetryMessage::AddPanel {
                    cockpit_name,
                    panel,
                }) => {
                    if let Some(ref mut cockpit) = self
                        .cockpits
                        .iter_mut()
                        .find(|c| c.get_name() == Some(&cockpit_name))
                    {
                        cockpit.add_panel(panel);
                    }
                    processed += 1;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let name = self
                        .name
                        .as_ref()
                        .map(|n| &**n)
                        .unwrap_or_else(|| "<no name>");
                    util::log_warning(format!(
                        "Processor '{}' failed to receive message. Channel disconnected. Exiting",
                        name
                    ));
                    self.is_disconnected = true;
                    break;
                }
            };
            num_received += 1;
        }

        let outcome = ProcessingOutcome {
            processed,
            dropped,
            instruments_updated,
        };

        if outcome.something_happened() {
            self.last_activity_at = Instant::now();
        }

        outcome
    }
}

impl<L> PutsSnapshot for TelemetryProcessor<L>
where
    L: Clone + Eq + Send + 'static,
{
    fn put_snapshot(&self, into: &mut Snapshot, descriptive: bool) {
        if let Some(ref name) = self.name {
            let mut new_level = Snapshot::default();
            self.put_values_into_snapshot(&mut new_level, descriptive);
            into.items
                .push((name.clone(), ItemKind::Snapshot(new_level)));
        } else {
            self.put_values_into_snapshot(into, descriptive);
        }
    }
}

impl<L> Descriptive for TelemetryProcessor<L> {
    fn title(&self) -> Option<&str> {
        self.title.as_ref().map(|n| &**n)
    }

    fn description(&self) -> Option<&str> {
        self.description.as_ref().map(|n| &**n)
    }
}

/// A building block for grouping
pub struct ProcessorMount {
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    processors: Vec<Box<dyn ProcessesTelemetryMessages>>,
    snapshooters: Vec<Box<dyn PutsSnapshot>>,
    last_activity_at: Instant,
    max_inactivity_duration: Option<Duration>,
}

impl ProcessorMount {
    /// Creates a new instance.
    ///
    /// Even though a name is optional having one
    /// is the default since this struct is mostly used to group
    /// other components.
    pub fn new<T: Into<String>>(name: T) -> ProcessorMount {
        let mut mount = ProcessorMount::default();
        mount.set_name(name);
        mount
    }

    /// Returns the name
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|n| &**n)
    }

    /// Sets the name of this `ProcessorMount`
    pub fn set_name<T: Into<String>>(&mut self, name: T) {
        self.name = Some(name.into())
    }

    /// Sets the maximum amount of time this processor may be
    /// inactive until no more snapshots are taken
    pub fn set_inactivity_limit(&mut self, limit: Duration) {
        self.max_inactivity_duration = Some(limit);
    }

    /// Returns the processors in this `ProcessorMount`
    pub fn processors(&self) -> Vec<&dyn ProcessesTelemetryMessages> {
        self.processors.iter().map(|p| &**p).collect()
    }

    /// Returns the snapshooters of this `ProcessorMount`
    pub fn snapshooters(&self) -> Vec<&dyn PutsSnapshot> {
        self.snapshooters.iter().map(|s| &**s).collect()
    }

    fn put_values_into_snapshot(&self, into: &mut Snapshot, descriptive: bool) {
        util::put_default_descriptives(self, into, descriptive);

        if let Some(d) = self.max_inactivity_duration {
            if self.last_activity_at.elapsed() > d {
                into.items
                    .push(("_inactive".to_string(), ItemKind::Boolean(true)));
                into.items
                    .push(("_active".to_string(), ItemKind::Boolean(false)));
                return;
            } else {
                into.items
                    .push(("_inactive".to_string(), ItemKind::Boolean(false)));
                into.items
                    .push(("_active".to_string(), ItemKind::Boolean(true)));
            }
        };

        self.processors
            .iter()
            .for_each(|p| p.put_snapshot(into, descriptive));

        self.snapshooters
            .iter()
            .for_each(|s| s.put_snapshot(into, descriptive));
    }
}

impl Default for ProcessorMount {
    fn default() -> ProcessorMount {
        ProcessorMount {
            name: None,
            title: None,
            description: None,
            processors: Vec::new(),
            snapshooters: Vec::new(),
            last_activity_at: Instant::now(),
            max_inactivity_duration: None,
        }
    }
}

impl AggregatesProcessors for ProcessorMount {
    fn add_processor<P: ProcessesTelemetryMessages>(&mut self, processor: P) {
        self.processors.push(Box::new(processor));
    }

    fn add_snapshooter<S: PutsSnapshot>(&mut self, snapshooter: S) {
        self.snapshooters.push(Box::new(snapshooter));
    }
}

impl ProcessesTelemetryMessages for ProcessorMount {
    fn process(&mut self, max: usize, strategy: ProcessingStrategy) -> ProcessingOutcome {
        let mut outcome = ProcessingOutcome::default();

        for processor in self.processors.iter_mut() {
            outcome.combine_with(&processor.process(max, strategy));
        }

        if outcome.something_happened() {
            self.last_activity_at = Instant::now();
        }

        outcome
    }
}

impl PutsSnapshot for ProcessorMount {
    fn put_snapshot(&self, into: &mut Snapshot, descriptive: bool) {
        if let Some(ref name) = self.name {
            let mut new_level = Snapshot::default();
            self.put_values_into_snapshot(&mut new_level, descriptive);
            into.items
                .push((name.clone(), ItemKind::Snapshot(new_level)));
        } else {
            self.put_values_into_snapshot(into, descriptive);
        }
    }
}

impl Descriptive for ProcessorMount {
    fn title(&self) -> Option<&str> {
        self.title.as_ref().map(|n| &**n)
    }

    fn description(&self) -> Option<&str> {
        self.description.as_ref().map(|n| &**n)
    }
}
