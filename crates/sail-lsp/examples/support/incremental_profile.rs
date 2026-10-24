//! Optional inclusive query timing/allocation probe. Profiling changes timing;
//! use ordinary incremental_stats runs for latency comparisons.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use tracing::{span::Id, Subscriber};
use tracing_subscriber::{
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
    Layer,
};
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn count(bytes: usize) {
    if ENABLED.load(Ordering::Relaxed) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
struct Sample {
    start: Instant,
    allocs: u64,
    bytes: u64,
}
impl Sample {
    fn now() -> Self {
        Self {
            start: Instant::now(),
            allocs: ALLOCS.load(Ordering::Relaxed),
            bytes: BYTES.load(Ordering::Relaxed),
        }
    }
}
#[derive(Default)]
struct Stats {
    calls: u64,
    us: u128,
    allocs: u64,
    bytes: u64,
}
#[derive(Clone, Default)]
pub struct Profiler(Arc<Mutex<BTreeMap<&'static str, Stats>>>);
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Profiler {
    fn on_enter(&self, id: &Id, cx: Context<'_, S>) {
        cx.span(id).unwrap().extensions_mut().insert(Sample::now());
    }
    fn on_exit(&self, id: &Id, cx: Context<'_, S>) {
        let span = cx.span(id).unwrap();
        let sample = span.extensions_mut().remove::<Sample>().unwrap();
        let elapsed = sample.start.elapsed().as_micros();
        let allocs = ALLOCS.load(Ordering::Relaxed) - sample.allocs;
        let bytes = BYTES.load(Ordering::Relaxed) - sample.bytes;
        let mut all = self.0.lock().unwrap();
        let stat = all.entry(span.name()).or_default();
        stat.calls += 1;
        stat.us += elapsed;
        stat.allocs += allocs;
        stat.bytes += bytes;
    }
}
impl Profiler {
    pub fn install(enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        let profiler = Self::default();
        let filter = tracing_subscriber::filter::filter_fn(|m| {
            m.is_span()
                && matches!(
                    m.name(),
                    "infer"
                        | "top_level_env"
                        | "local_environment"
                        | "callable_names"
                        | "workspace_context"
                        | "file_symbols"
                        | "workspace_selection"
                        | "callable_environment"
                        | "file_contributions"
                        | "symbol_locations"
                        | "workspace_symbol"
                )
        });
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(profiler.clone().with_filter(filter)),
        )
        .unwrap();
        Some(profiler)
    }
    pub fn reset(&self) {
        self.0.lock().unwrap().clear();
        ALLOCS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
    }
    pub fn report(&self, phase: &str) {
        ENABLED.store(false, Ordering::Relaxed);
        println!("METRIC:profile_{phase}_allocations:{}:count", ALLOCS.load(Ordering::Relaxed));
        println!("METRIC:profile_{phase}_allocated:{}:bytes", BYTES.load(Ordering::Relaxed));
        for (name, stat) in self.0.lock().unwrap().iter() {
            println!(
                "PROFILE:{phase}:{name}:calls={}:us={}:allocations={}:bytes={}",
                stat.calls, stat.us, stat.allocs, stat.bytes
            );
        }
    }
}
