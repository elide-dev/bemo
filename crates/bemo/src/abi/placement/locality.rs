use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Domain {
  node: Option<usize>,
  cache: Option<Vec<usize>>,
}

fn ranges(value: &str) -> Option<Vec<(usize, usize)>> {
  value
    .trim()
    .split(',')
    .map(|part| {
      let (first, last) = part.split_once('-').unwrap_or((part, part));
      let first = first.parse().ok()?;
      let last = last.parse().ok()?;
      (first <= last).then_some((first, last))
    })
    .collect()
}

fn cache_domain(path: &Path, cores: &[Vec<usize>], own: usize) -> Option<Vec<usize>> {
  let mut caches = Vec::new();
  for entry in std::fs::read_dir(path.join("cache")).ok()? {
    let entry = entry.ok()?;
    if !entry.file_name().to_str()?.starts_with("index") {
      continue;
    }
    let path = entry.path();
    let kind = std::fs::read_to_string(path.join("type")).ok()?;
    if !matches!(kind.trim(), "Unified" | "Data") {
      continue;
    }
    let level: u32 = std::fs::read_to_string(path.join("level")).ok()?.trim().parse().ok()?;
    caches.push((level, path));
  }
  let level = caches.iter().map(|(level, _)| *level).max()?;
  let mut domain = None;
  for (_, path) in caches.iter().filter(|(candidate, _)| *candidate == level) {
    let mask = ranges(&std::fs::read_to_string(path.join("shared_cpu_list")).ok()?)?;
    let mut members = Vec::new();
    for (index, siblings) in cores.iter().enumerate() {
      let included = siblings
        .iter()
        .filter(|cpu| mask.iter().any(|(first, last)| (first..=last).contains(cpu)))
        .count();
      if included != 0 {
        if included != siblings.len() {
          return None;
        }
        members.push(index);
      }
    }
    if !members.contains(&own) || domain.as_ref().is_some_and(|prior| prior != &members) {
      return None;
    }
    domain = Some(members);
  }
  domain
}

fn node_domain(path: &Path) -> Option<usize> {
  let mut nodes = Vec::new();
  for entry in std::fs::read_dir(path).ok()? {
    let entry = entry.ok()?;
    if let Some(node) = entry.file_name().to_str()?.strip_prefix("node") {
      nodes.push(node.parse().ok()?);
    }
  }
  if nodes.len() == 1 { nodes.pop() } else { None }
}

fn discover(cores: &[Vec<usize>], root: &Path) -> Vec<Domain> {
  let mut domains: Vec<_> = cores
    .iter()
    .enumerate()
    .map(|(index, core)| {
      let path = root.join(format!("cpu{}", core[0]));
      Domain {
        node: node_domain(&path),
        cache: cache_domain(&path, cores, index),
      }
    })
    .collect();
  let complete_nodes = domains.iter().all(|domain| domain.node.is_some());
  let complete_caches = domains.iter().all(|domain| {
    domain
      .cache
      .as_ref()
      .is_some_and(|members| members.iter().all(|&member| domains[member].cache == domain.cache))
  });
  for domain in &mut domains {
    if !complete_nodes {
      domain.node = None;
    }
    if !complete_caches {
      domain.cache = None;
    }
  }
  domains
}

#[cfg(target_os = "linux")]
pub(super) fn assign(cores: Vec<Vec<usize>>, contexts: usize, workers: usize) -> Vec<Vec<usize>> {
  let domains = discover(&cores, Path::new("/sys/devices/system/cpu"));
  plan(cores, &domains, contexts, workers)
}

struct Groups {
  ids: Vec<Option<usize>>,
  loads: Vec<(usize, usize)>,
}

impl Groups {
  fn new(keys: impl Iterator<Item = Option<usize>>) -> Self {
    let mut groups = std::collections::BTreeMap::new();
    let mut loads = Vec::new();
    let ids = keys
      .map(|key| {
        key.map(|key| {
          let next = groups.len();
          let id = *groups.entry(key).or_insert(next);
          if id == loads.len() {
            loads.push((0, 0));
          }
          loads[id].1 += 1;
          id
        })
      })
      .collect();
    Self { ids, loads }
  }

  fn load(&self, core: usize) -> (usize, usize) {
    self.ids[core].map_or((0, 0), |id| self.loads[id])
  }

  fn claim(&mut self, core: usize) {
    if let Some(id) = self.ids[core] {
      self.loads[id].0 += 1;
    }
  }

  fn shared(&self, first: usize, second: usize) -> bool {
    self.ids[first].is_some() && self.ids[first] == self.ids[second]
  }
}

fn plan(cores: Vec<Vec<usize>>, domains: &[Domain], contexts: usize, workers: usize) -> Vec<Vec<usize>> {
  assert!(contexts > 0 && contexts <= workers && workers <= cores.len());
  assert_eq!(cores.len(), domains.len());
  if domains.iter().all(|domain| domain == &domains[0]) {
    return cores;
  }
  let mut nodes = Groups::new(domains.iter().map(|domain| domain.node));
  let mut caches = Groups::new(
    domains
      .iter()
      .map(|domain| domain.cache.as_ref().and_then(|members| members.first().copied())),
  );
  let mut used = vec![false; cores.len()];
  let mut indices = vec![0; workers];
  for context in 0..contexts {
    let size = (workers - 1 - context) / contexts + 1;
    let score = |index: usize| {
      let cache = caches.load(index);
      let node = nodes.load(index);
      let spill = if cache.1 - cache.0 >= size {
        0
      } else if node.1 - node.0 >= size {
        1
      } else {
        2
      };
      (spill, node, cache)
    };
    let primary = (0..cores.len())
      .filter(|&index| !used[index])
      .min_by(|&a, &b| {
        let (as_, an, ac) = score(a);
        let (bs, bn, bc) = score(b);
        let load = |a: (usize, usize), b: (usize, usize)| {
          ((a.0 as u128) * b.1.max(1) as u128).cmp(&((b.0 as u128) * a.1.max(1) as u128))
        };
        as_
          .cmp(&bs)
          .then_with(|| load(an, bn))
          .then_with(|| load(ac, bc))
          .then_with(|| a.cmp(&b))
      })
      .unwrap();
    indices[context] = primary;
    used[primary] = true;
    nodes.claim(primary);
    caches.claim(primary);
    for slot in ((context + contexts)..workers).step_by(contexts) {
      let extra = (0..cores.len())
        .filter(|&index| !used[index])
        .min_by_key(|&index| {
          let distance = if caches.shared(primary, index) {
            0
          } else if nodes.shared(primary, index) {
            1
          } else {
            2
          };
          (distance, index)
        })
        .unwrap();
      indices[slot] = extra;
      used[extra] = true;
      nodes.claim(extra);
      caches.claim(extra);
    }
  }
  indices.extend((0..cores.len()).filter(|&index| !used[index]));
  indices.into_iter().map(|index| cores[index].clone()).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  struct Fixture(std::path::PathBuf);
  impl Fixture {
    fn new() -> Self {
      static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
      let path = std::env::temp_dir().join(format!(
        "elide-locality-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
      ));
      std::fs::create_dir(&path).unwrap();
      Self(path)
    }
    fn cpu(&self, cpu: usize, node: usize, cache: &str) {
      let path = self.0.join(format!("cpu{cpu}"));
      std::fs::create_dir_all(path.join(format!("node{node}"))).unwrap();
      let cache_path = path.join("cache/index7");
      std::fs::create_dir_all(&cache_path).unwrap();
      for (name, value) in [("level", "3"), ("type", "Unified"), ("shared_cpu_list", cache)] {
        std::fs::write(cache_path.join(name), value).unwrap();
      }
    }
  }
  impl Drop for Fixture {
    fn drop(&mut self) {
      std::fs::remove_dir_all(&self.0).unwrap();
    }
  }

  #[test]
  fn discovers_cache_sharing_without_assuming_cache_index_or_dense_cpus() {
    let fixture = Fixture::new();
    fixture.cpu(3, 2, "0-3,11");
    fixture.cpu(8, 7, "8,19-20");
    fixture.cpu(19, 7, "8,19-20");
    let cores = vec![vec![3, 11], vec![8], vec![19]];
    assert_eq!(
      discover(&cores, &fixture.0),
      vec![
        Domain {
          node: Some(2),
          cache: Some(vec![0])
        },
        Domain {
          node: Some(7),
          cache: Some(vec![1, 2])
        },
        Domain {
          node: Some(7),
          cache: Some(vec![1, 2])
        },
      ]
    );
  }

  #[test]
  fn incomplete_or_inconsistent_cache_data_retains_only_numa_locality() {
    for invalid in ["", "broken", "4-1", "1", "0-1"] {
      let fixture = Fixture::new();
      fixture.cpu(0, 0, invalid);
      fixture.cpu(1, 1, "1");
      assert_eq!(
        discover(&[vec![0], vec![1]], &fixture.0),
        vec![
          Domain {
            node: Some(0),
            cache: None
          },
          Domain {
            node: Some(1),
            cache: None
          },
        ]
      );
    }
  }

  #[test]
  fn unavailable_sysfs_falls_back_without_failing_startup() {
    let fixture = Fixture::new();
    assert_eq!(discover(&[vec![0], vec![4]], &fixture.0), vec![Domain::default(); 2]);
  }

  fn domains(nodes: &[usize], caches: &[usize]) -> Vec<Domain> {
    nodes
      .iter()
      .zip(caches)
      .map(|(&node, &cache)| Domain {
        node: Some(node),
        cache: Some(vec![cache]),
      })
      .collect()
  }

  #[test]
  fn secondary_owners_stay_with_their_context_and_groups_spread_across_caches() {
    let cores = vec![vec![0, 8], vec![1, 9], vec![2, 10], vec![3, 11]];
    let placed = plan(cores, &domains(&[0; 4], &[0, 0, 1, 1]), 2, 4);
    assert_eq!(placed, vec![vec![0, 8], vec![2, 10], vec![1, 9], vec![3, 11]]);
  }

  #[test]
  fn sparse_single_sibling_masks_never_gain_cpus() {
    let placed = plan(
      vec![vec![3], vec![8], vec![11], vec![19]],
      &domains(&[0; 4], &[0, 0, 1, 1]),
      2,
      4,
    );
    assert_eq!(placed, vec![vec![3], vec![11], vec![8], vec![19]]);
  }

  #[test]
  fn overflowing_cache_stays_in_the_same_numa_node() {
    let cores = (0..8).map(|cpu| vec![cpu]).collect();
    let placed = plan(
      cores,
      &domains(&[0, 0, 1, 1, 0, 0, 1, 1], &[0, 0, 1, 1, 2, 2, 3, 3]),
      1,
      4,
    );
    assert_eq!(&placed[..4], &[vec![0], vec![1], vec![4], vec![5]]);
  }

  #[test]
  fn independent_groups_balance_numa_nodes_before_caches() {
    let cores = (0..8).map(|cpu| vec![cpu]).collect();
    let placed = plan(
      cores,
      &domains(&[0, 0, 0, 0, 1, 1, 1, 1], &[0, 0, 1, 1, 2, 2, 3, 3]),
      2,
      4,
    );
    assert_eq!(&placed[..4], &[vec![0], vec![4], vec![1], vec![5]]);
  }

  #[test]
  fn unequal_domain_capacity_balances_fractional_occupancy() {
    let cores = (0..8).map(|cpu| vec![cpu]).collect();
    let placed = plan(
      cores,
      &domains(&[0, 0, 1, 1, 1, 1, 1, 1], &[0, 0, 1, 1, 1, 1, 1, 1]),
      4,
      4,
    );
    assert_eq!(&placed[..4], &[vec![0], vec![2], vec![3], vec![4]]);
  }

  #[test]
  fn unknown_topology_preserves_existing_order() {
    let cores = vec![vec![3, 11], vec![8], vec![19]];
    assert_eq!(plan(cores.clone(), &vec![Domain::default(); 3], 2, 3), cores);
  }
}
