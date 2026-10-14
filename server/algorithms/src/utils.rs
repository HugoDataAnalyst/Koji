use std::collections::{HashMap, VecDeque};
use std::fmt::Debug;
use std::fs;
use std::fs::{File, create_dir_all};
use std::io::Write;
use std::path::Path;

use colored::Colorize;
use geo::Coord;
use geohash::encode;
use hashbrown::HashSet;
use model::api::Precision;
use model::api::{point_array::PointArray, single_vec::SingleVec};

use crate::rtree::cluster::Cluster;
use crate::stats::Stats;

pub fn debug_hashmap<T, U>(file_name: &str, input: &T) -> std::io::Result<()>
where
    U: Debug,
    T: Debug + Clone + IntoIterator<Item = U>,
{
    create_dir_all("./debug_files")?;
    let path = format!("./debug_files/{}", file_name);
    let mut content: String = "".to_string();

    for x in input.clone().into_iter() {
        content = format!("{}\n{:?}\n", content, x);
    }
    content = content.trim_end_matches(",").to_string();
    let mut output = File::create(path)?;
    write!(output, "{}", content)?;
    // log::info!("Saved {} to file with {} coords", file_name, input.len());
    Ok(())
}

pub fn debug_string(file_name: &str, input: &String) -> std::io::Result<()> {
    create_dir_all("./debug_files")?;
    let path = format!("./debug_files/{}", file_name);
    let mut output = File::create(path)?;
    write!(output, "{}", input)?;
    // log::info!("Saved {} to file with {} coords", file_name, input.len());
    Ok(())
}

pub fn get_sorted<T>(map: &HashMap<String, T>) -> Vec<(String, T)>
where
    T: Clone,
{
    let mut vec: Vec<&String> = map.keys().collect();
    vec.sort();
    vec.into_iter()
        .map(|k| (k.clone(), map.get(k).unwrap().clone()))
        .collect()
}

pub fn centroid(coords: &[[Precision; 2]]) -> PointArray {
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);

    for loc in coords.iter() {
        let lat = loc[0].to_radians();
        let lon = loc[1].to_radians();

        x += lat.cos() * lon.cos();
        y += lat.cos() * lon.sin();
        z += lat.sin();
    }

    let number_of_locations = coords.len() as Precision;
    x /= number_of_locations;
    y /= number_of_locations;
    z /= number_of_locations;

    let hyp = (x * x + y * y).sqrt();
    let lon = y.atan2(x);
    let lat = z.atan2(hyp);

    [lat.to_degrees(), lon.to_degrees()]
}

pub fn info_log(file_name: &str, message: String) -> String {
    format!(
        "\r{}{}Z {}  {}{} {}",
        "[".black(),
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
        "INFO".green(),
        file_name,
        "]".black(),
        message
    )
}

pub fn _debug_clusters(clusters: &HashSet<Cluster>, file_suffix: &str) {
    let mut point_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut cluster_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut unique_map: HashMap<String, HashSet<String>> = HashMap::new();

    for cluster in clusters.iter() {
        cluster_map.insert(
            cluster.point._get_geohash(),
            cluster.all.iter().map(|p| p._get_geohash()).collect(),
        );
        unique_map.insert(
            cluster.point._get_geohash(),
            cluster.unique.iter().map(|p| p._get_geohash()).collect(),
        );
        for point in cluster.all.iter() {
            point_map
                .entry(point._get_geohash())
                .and_modify(|f| {
                    f.insert(cluster.point._get_geohash());
                })
                .or_insert_with(|| {
                    let mut set: HashSet<String> = HashSet::new();
                    set.insert(cluster.point._get_geohash());
                    set
                });
        }
    }

    debug_hashmap(&format!("{}_point.txt", file_suffix), &point_map).unwrap();
    debug_hashmap(&format!("{}_cluster.txt", file_suffix), &cluster_map).unwrap();
    debug_hashmap(&format!("{}_unique.txt", file_suffix), &unique_map).unwrap();
}

/// Default CPU ceiling (percent of ALL cores combined) when neither the request
/// nor `KOJI_HOG_CPU` says otherwise.
pub const DEFAULT_HOG_CPU: u8 = 90;

/// Resolve the effective `hog_cpu` percentage: the request value if set (1..=100),
/// else `KOJI_HOG_CPU`, else [`DEFAULT_HOG_CPU`].
pub fn resolve_hog_cpu(requested: u8) -> u8 {
    let from_env = || {
        std::env::var("KOJI_HOG_CPU")
            .ok()
            .and_then(|v| v.trim().trim_end_matches('%').parse::<u8>().ok())
    };
    let pct = if requested > 0 {
        requested
    } else {
        from_env().unwrap_or(DEFAULT_HOG_CPU)
    };
    pct.clamp(1, 100)
}

/// Busy cores used by everything else on the host, from a ~200 ms sample of
/// global CPU usage (cached for 3 s so back-to-back jobs don't each wait).
fn other_busy_cores(cores: usize) -> f64 {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, f64)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *cache {
        if at.elapsed() < Duration::from_secs(3) {
            return v;
        }
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_usage();
    let busy = sys.global_cpu_usage() as f64 / 100.0 * cores as f64;
    *cache = Some((Instant::now(), busy));
    busy
}

/// Threads a calculation may use so that the whole host stays at or under
/// `hog_cpu` percent: `floor(cores * pct/100 - other load)`, at least 1.
/// `100` skips the load probe and uses every core.
pub fn cpu_budget(hog_cpu: u8) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |c| c.get());
    let pct = hog_cpu.clamp(1, 100);
    if pct >= 100 {
        return cores;
    }
    let allowed = cores as f64 * pct as f64 / 100.0;
    let free = allowed - other_busy_cores(cores);
    let t = (free.floor() as isize).clamp(1, cores as isize) as usize;
    log::info!("hog_cpu {pct}%: {cores} cores, other load leaves {t} thread(s)");
    t
}

/// Run `f` on a rayon pool sized by [`cpu_budget`], so every `par_iter` inside
/// (clustering, routing) and the TSP solver (which reads the pool size) stay
/// under the CPU ceiling. `KOJI_CALC_THREADS` additionally caps the thread count.
pub fn with_thread_cap<R: Send>(hog_cpu: u8, f: impl FnOnce() -> R + Send) -> R {
    let mut threads = cpu_budget(resolve_hog_cpu(hog_cpu));
    if let Some(cap) = std::env::var("KOJI_CALC_THREADS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|t| *t > 0)
    {
        threads = threads.min(cap);
    }
    match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
        Ok(pool) => pool.install(f),
        Err(_) => f(),
    }
}

pub fn rotate_to_best(clusters: SingleVec, stats: &Stats) -> SingleVec {
    let mut final_clusters = VecDeque::<PointArray>::new();

    let best_cluster_set = stats
        .best_clusters
        .iter()
        .map(|x| encode(Coord { x: x[1], y: x[0] }, 12).unwrap())
        .collect::<HashSet<String>>();
    let mut rotate_count = 0;
    for (i, [lat, lon]) in clusters.into_iter().enumerate() {
        if best_cluster_set.contains(&encode(Coord { x: lon, y: lat }, 12).unwrap()) {
            rotate_count = i;
            log::debug!("Found Best! {}, {} - {}", lat, lon, i);
        }
        final_clusters.push_back([lat, lon]);
    }
    final_clusters.rotate_left(rotate_count);

    final_clusters.into()
}

pub fn get_plugin_list(path: &str) -> std::io::Result<Vec<String>> {
    let path = Path::new(path);

    fs::read_dir(path)?
        .map(|res| res.map(|e| e.path().display().to_string()))
        .filter_map(|path| {
            if let Ok(ext) = path {
                let plugin = ext
                    .split(std::path::MAIN_SEPARATOR_STR)
                    .last()
                    .unwrap_or("")
                    .to_string();
                if plugin == ".gitkeep" {
                    None
                } else {
                    Some(Ok(plugin))
                }
            } else {
                None
            }
        })
        .collect::<Result<Vec<_>, std::io::Error>>()
}

pub fn stringify_points(points: &SingleVec) -> String {
    points
        .iter()
        .enumerate()
        .map(|(i, cluster)| {
            format!(
                "{},{}{}",
                cluster[0],
                cluster[1],
                if i == points.len() - 1 { "" } else { " " }
            )
        })
        .collect()
}
