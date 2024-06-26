use crate::meta_map::{MetaMap, Metadata, PslHint};
use crate::{Map, Probe, Update};
use ahash::RandomState;

// dummy hash-set for u64 keys.
//
// implements robin-hood-hashing with backward-shift deletion
pub struct RobinHood {
    hasher: RandomState,
    buckets: Vec<Option<u64>>,
    meta: MetaMap,
    len: usize,
}

impl RobinHood {
    pub fn new(capacity: usize, meta_bits: usize) -> Self {
        RobinHood {
            hasher: RandomState::new(),
            buckets: vec![None; capacity],
            meta: MetaMap::new(capacity, meta_bits),
            len: 0,
        }
    }

    fn bucket_for(&self, key: u64) -> usize {
        (self.hasher.hash_one(key) % (self.buckets.len() as u64)) as usize
    }

    fn psl_of(&self, key: u64, bucket: usize) -> usize {
        let home = self.bucket_for(key);
        1 + if bucket < home {
            (bucket + self.buckets.len()) - home
        } else {
            bucket - home
        }
    }

    fn set_bucket(&mut self, bucket: usize, key: u64, psl: usize) {
        self.buckets[bucket] = Some(key);
        self.meta.set_full(bucket, Metadata::Psl(psl));
    }

    fn clear_bucket(&mut self, bucket: usize) {
        self.buckets[bucket] = None;
        self.meta.set_empty(bucket);
    }
}

impl Map for RobinHood {
    fn len(&self) -> usize {
        self.len
    }

    fn capacity(&self) -> usize {
        self.buckets.len()
    }

    fn probe(&self, key: u64) -> Probe {
        let mut psl = 1;
        let mut probes = 0;

        let mut bucket = self.bucket_for(key);
        loop {
            match self.meta.hint_psl(bucket) {
                None if self.meta.hint_empty(bucket) => {
                    return Probe {
                        contained: false,
                        probes,
                    }
                }
                None => {}
                Some(PslHint::Exact(bucket_psl)) => {
                    if bucket_psl < psl {
                        return Probe {
                            contained: false,
                            probes,
                        };
                    } else if bucket_psl > psl {
                        psl += 1;
                        bucket = (bucket + 1) % self.buckets.len();
                        continue;
                    }
                }
                Some(PslHint::AtLeast(bucket_psl)) => {
                    if bucket_psl > psl {
                        psl += 1;
                        bucket = (bucket + 1) % self.buckets.len();
                        continue;
                    }
                }
            }

            probes += 1;
            match self.buckets[bucket] {
                None => {
                    return Probe {
                        contained: false,
                        probes,
                    }
                }
                Some(k) if k == key => {
                    return Probe {
                        contained: true,
                        probes,
                    }
                }
                Some(k) => {
                    if self.psl_of(k, bucket) < psl {
                        return Probe {
                            contained: false,
                            probes,
                        };
                    }
                }
            }

            psl += 1;
            bucket = (bucket + 1) % self.buckets.len()
        }
    }

    fn insert(&mut self, key: u64) -> Update {
        let mut update = Update {
            total_probes: 0,
            total_writes: 1,
            completed: true,
        };

        let mut home_bucket = self.bucket_for(key);
        let mut active_key = key;
        let mut psl = 1;
        self.len += 1;

        loop {
            let bucket = (home_bucket + psl - 1) % self.buckets.len();

            let skip = match self.meta.hint_psl(bucket) {
                None if self.meta.hint_empty(bucket) => {
                    self.set_bucket(bucket, active_key, psl);
                    return update;
                }
                None => false,
                Some(PslHint::Exact(bucket_psl)) => bucket_psl >= psl,
                Some(PslHint::AtLeast(bucket_psl)) => bucket_psl >= psl,
            };

            if skip {
                psl += 1;
                continue;
            }

            update.total_probes += 1;
            if self.buckets[bucket].is_none() {
                self.set_bucket(bucket, active_key, psl);
                return update;
            }

            let contained_key = self.buckets[bucket].unwrap();
            if contained_key == active_key {
                if active_key == key {
                    self.len -= 1;
                }
                return update;
            }

            let contained_home = self.bucket_for(contained_key);
            let contained_psl = self.psl_of(contained_key, bucket);

            if contained_psl < psl {
                self.set_bucket(bucket, active_key, psl);

                home_bucket = contained_home;
                active_key = contained_key;
                psl = contained_psl;
                update.total_writes += 1;
            }

            psl += 1;
        }
    }

    fn remove(&mut self, key: u64) -> Update {
        let probe = self.probe(key);
        let mut update = Update {
            total_probes: probe.probes,
            total_writes: 0,
            completed: true,
        };

        if !probe.contained {
            return update;
        }

        self.len -= 1;

        let mut bucket = (self.bucket_for(key) + probe.probes - 1) % self.buckets.len();
        self.clear_bucket(bucket);
        update.total_writes += 1;

        loop {
            let next_bucket = (bucket + 1) % self.buckets.len();

            if let Some(PslHint::Exact(1)) = self.meta.hint_psl(next_bucket) {
                return update;
            }

            update.total_probes += 1;
            let (shift_key, shift_psl) = match self.buckets[next_bucket] {
                None => return update,
                Some(k) => {
                    let shift_psl = self.psl_of(k, next_bucket);
                    if shift_psl == 1 {
                        return update;
                    }

                    self.clear_bucket(next_bucket);
                    (k, shift_psl - 1)
                }
            };

            self.set_bucket(bucket, shift_key, shift_psl);
            bucket = next_bucket;
            update.total_writes += 1;
        }
    }
}
