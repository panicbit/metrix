use std::time::{Duration, Instant};

use crate::snapshot::{ItemKind, Snapshot};
use crate::util;
use crate::{Descriptive, HandlesObservations, Observation, PutsSnapshot};

use super::*;

/// The panel shows recorded
/// observations with the same label
/// in different representations.
///
/// Let's say you want to monitor the successful requests
/// of a specific endpoint of your REST API.
/// You would then create a panel for this and might
/// want to add a counter and a meter and a histogram
/// to track latencies.
///
/// # Example
///
/// ```
/// use std::time::Instant;
/// use metrix::instruments::*;
/// use metrix::{HandlesObservations, Observation};
///
/// #[derive(Clone, PartialEq, Eq)]
/// struct SuccessfulRequests;
///
/// let counter = Counter::new_with_defaults("count");
/// let gauge = Gauge::new_with_defaults("last_latency");
/// let meter = Meter::new_with_defaults("per_second");
/// let histogram = Histogram::new_with_defaults("latencies");
///
/// assert_eq!(0, counter.get());
/// assert_eq!(None, gauge.get());
///
/// let mut panel = Panel::named(SuccessfulRequests, "successful_requests");
/// panel.add_counter(counter);
/// panel.add_gauge(gauge);
/// panel.add_meter(meter);
/// panel.add_histogram(histogram);
///
/// let observation = Observation::ObservedOneValue {
///        label: SuccessfulRequests,
///        value: 12.into(),
///        timestamp: Instant::now(),
/// };
/// panel.handle_observation(&observation);
/// ```
pub struct Panel<L> {
    label_filter: LabelFilter<L>,
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    counter: Option<InstrumentAdapter<L, Counter>>,
    gauge: Option<GaugeAdapter<L>>,
    meter: Option<InstrumentAdapter<L, Meter>>,
    histogram: Option<InstrumentAdapter<L, Histogram>>,
    panels: Vec<Panel<L>>,
    handlers: Vec<Box<dyn HandlesObservations<Label = L>>>,
    snapshooters: Vec<Box<dyn PutsSnapshot>>,
    last_update: Instant,
    max_inactivity_duration: Option<Duration>,
}

impl<L> Panel<L>
where
    L: Eq + Send + 'static,
{
    /// Create a new `Panel` without a name which dispatches observations
    /// with the given label
    pub fn new<F: Into<LabelFilter<L>>>(accept: F) -> Panel<L> {
        Panel {
            label_filter: accept.into(),
            name: None,
            title: None,
            description: None,
            counter: None,
            gauge: None,
            meter: None,
            histogram: None,
            panels: Vec::new(),
            handlers: Vec::new(),
            snapshooters: Vec::new(),
            last_update: Instant::now(),
            max_inactivity_duration: None,
        }
    }

    /// Create a new `Panel` with the given name which dispatches observations
    /// with the given label
    pub fn named<T: Into<String>, F: Into<LabelFilter<L>>>(accept: F, name: T) -> Panel<L> {
        let mut panel = Self::new(accept);
        panel.name = Some(name.into());
        panel
    }

    /// Create a new `Panel` with the given name which dispatches observations
    /// with the given label
    #[deprecated(since = "0.9.24", note = "use 'named'")]
    pub fn with_name<T: Into<String>>(label: L, name: T) -> Panel<L> {
        Self::named(label, name)
    }

    /// Create a new `Panel` without a name which dispatches observations
    /// with the given labels
    pub fn accept<F: Into<LabelFilter<L>>>(accept: F) -> Self {
        Self::new(accept)
    }

    /// Create a new `Panel` with the given name which dispatches observations
    /// with the given labels
    pub fn accept_named<T: Into<String>, F: Into<LabelFilter<L>>>(accept: F, name: T) -> Self {
        Self::named(accept, name)
    }

    /// Create a new `Panel` with the given name which dispatches all
    /// observations
    pub fn accept_all_named<T: Into<String>>(name: T) -> Panel<L> {
        Self::named(AcceptAllLabels, name)
    }

    /// Create a new `Panel` without a name which dispatches all
    /// observations
    pub fn accept_all() -> Panel<L> {
        Self::new(AcceptAllLabels)
    }

    #[deprecated(since = "0.10.9", note = "use 'add_histogram'")]
    pub fn set_counter<I: Into<InstrumentAdapter<L, Counter>>>(&mut self, counter: I) {
        self.counter = Some(counter.into());
    }

    pub fn add_counter<I: Into<InstrumentAdapter<L, Counter>>>(&mut self, counter: I) {
        if self.counter.is_none() {
            self.counter = Some(counter.into());
        } else {
            self.add_handler(counter.into())
        }
    }

    pub fn counter<I: Into<InstrumentAdapter<L, Counter>>>(mut self, counter: I) -> Self {
        self.add_counter(counter);
        self
    }

    #[deprecated(since = "0.10.9", note = "use 'add_gauge'")]
    pub fn set_gauge<I: Into<GaugeAdapter<L>>>(&mut self, gauge: I) {
        self.gauge = Some(gauge.into());
    }

    pub fn gauge<I: Into<GaugeAdapter<L>>>(mut self, gauge: I) -> Self {
        self.add_gauge(gauge);
        self
    }

    pub fn add_gauge<I: Into<GaugeAdapter<L>>>(&mut self, gauge: I) {
        if self.gauge.is_none() {
            self.gauge = Some(gauge.into());
        } else {
            self.add_handler(gauge.into())
        }
    }

    #[deprecated(since = "0.10.9", note = "use 'add_meter'")]
    pub fn set_meter<I: Into<InstrumentAdapter<L, Meter>>>(&mut self, meter: I) {
        self.meter = Some(meter.into());
    }

    pub fn meter<I: Into<InstrumentAdapter<L, Meter>>>(mut self, meter: I) -> Self {
        self.add_meter(meter);
        self
    }

    pub fn add_meter<I: Into<InstrumentAdapter<L, Meter>>>(&mut self, meter: I) {
        if self.meter.is_none() {
            self.meter = Some(meter.into());
        } else {
            self.add_handler(meter.into())
        }
    }

    #[deprecated(since = "0.10.9", note = "use 'add_histogram'")]
    pub fn set_histogram<I: Into<InstrumentAdapter<L, Histogram>>>(&mut self, histogram: I) {
        self.histogram = Some(histogram.into());
    }

    pub fn add_histogram<I: Into<InstrumentAdapter<L, Histogram>>>(&mut self, histogram: I) {
        if self.histogram.is_none() {
            self.histogram = Some(histogram.into());
        } else {
            self.add_handler(histogram.into())
        }
    }

    pub fn histogram<I: Into<InstrumentAdapter<L, Histogram>>>(mut self, histogram: I) -> Self {
        self.add_histogram(histogram);
        self
    }

    pub fn add_snapshooter<T: PutsSnapshot>(&mut self, snapshooter: T) {
        self.snapshooters.push(Box::new(snapshooter));
    }

    pub fn snapshooter<T: PutsSnapshot>(mut self, snapshooter: T) -> Self {
        self.add_snapshooter(snapshooter);
        self
    }

    pub fn add_instrument<I: Instrument>(&mut self, instrument: I) {
        self.handlers
            .push(Box::new(InstrumentAdapter::new(instrument)));
    }

    pub fn instrument<T: Instrument>(mut self, instrument: T) -> Self {
        self.add_instrument(instrument);
        self
    }

    pub fn add_panel(&mut self, panel: Panel<L>) {
        self.panels.push(panel);
    }

    pub fn panel(mut self, panel: Panel<L>) -> Self {
        self.add_panel(panel);
        self
    }

    pub fn add_handler<H: HandlesObservations<Label = L>>(&mut self, handler: H) {
        self.handlers.push(Box::new(handler));
    }

    pub fn handler<H: HandlesObservations<Label = L>>(mut self, handler: H) -> Self {
        self.add_handler(handler);
        self
    }

    /// Gets the name of this `Panel`
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|n| &**n)
    }

    /// Set the name if this `Panel`.
    ///
    /// The name is a path segment within a `Snapshot`
    pub fn set_name<T: Into<String>>(&mut self, name: T) {
        self.name = Some(name.into());
    }

    /// Sets the `title` of this `Panel`.
    ///
    /// A title can be part of a descriptive `Snapshot`
    pub fn set_title<T: Into<String>>(&mut self, title: T) {
        self.title = Some(title.into())
    }

    /// Sets the `description` of this `Panel`.
    ///
    /// A description can be part of a descriptive `Snapshot`
    pub fn set_description<T: Into<String>>(&mut self, description: T) {
        self.description = Some(description.into())
    }

    /// Sets the maximum amount of time this panel may be
    /// inactive until no more snapshots are taken
    ///
    /// Default is no inactivity tracking.
    pub fn inactivity_limit(mut self, limit: Duration) -> Self {
        self.set_inactivity_limit(limit);
        self
    }

    /// Sets the maximum amount of time this panel may be
    /// inactive until no more snapshots are taken
    ///
    /// Default is no inactivity tracking.
    pub fn set_inactivity_limit(&mut self, limit: Duration) {
        self.max_inactivity_duration = Some(limit);
    }

    pub fn accepts_label(&self, label: &L) -> bool {
        self.label_filter.accepts(label)
    }

    fn put_values_into_snapshot(&self, into: &mut Snapshot, descriptive: bool) {
        util::put_default_descriptives(self, into, descriptive);
        if let Some(d) = self.max_inactivity_duration {
            if self.last_update.elapsed() > d {
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
        self.counter
            .as_ref()
            .iter()
            .for_each(|x| x.put_snapshot(into, descriptive));
        self.gauge
            .as_ref()
            .iter()
            .for_each(|x| x.put_snapshot(into, descriptive));
        self.meter
            .as_ref()
            .iter()
            .for_each(|x| x.put_snapshot(into, descriptive));
        self.histogram
            .as_ref()
            .iter()
            .for_each(|x| x.put_snapshot(into, descriptive));
        self.panels
            .iter()
            .for_each(|p| p.put_snapshot(into, descriptive));
        self.snapshooters
            .iter()
            .for_each(|p| p.put_snapshot(into, descriptive));
        self.handlers
            .iter()
            .for_each(|p| p.put_snapshot(into, descriptive));
    }
}

impl<L> PutsSnapshot for Panel<L>
where
    L: Eq + Send + 'static,
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

impl<L> HandlesObservations for Panel<L>
where
    L: Eq + Send + 'static,
{
    type Label = L;

    fn handle_observation(&mut self, observation: &Observation<Self::Label>) -> usize {
        if !self.label_filter.accepts(observation.label()) {
            return 0;
        }

        let mut instruments_updated = 0;

        self.counter
            .iter_mut()
            .for_each(|x| instruments_updated += x.handle_observation(&observation));
        self.gauge
            .iter_mut()
            .for_each(|x| instruments_updated += x.handle_observation(&observation));
        self.meter
            .iter_mut()
            .for_each(|x| instruments_updated += x.handle_observation(&observation));
        self.histogram
            .iter_mut()
            .for_each(|x| instruments_updated += x.handle_observation(&observation));
        self.panels
            .iter_mut()
            .for_each(|x| instruments_updated += x.handle_observation(&observation));
        self.handlers
            .iter_mut()
            .for_each(|x| instruments_updated += x.handle_observation(&observation));

        instruments_updated
    }
}

impl<L> Descriptive for Panel<L> {
    fn title(&self) -> Option<&str> {
        self.title.as_ref().map(|n| &**n)
    }

    fn description(&self) -> Option<&str> {
        self.description.as_ref().map(|n| &**n)
    }
}
