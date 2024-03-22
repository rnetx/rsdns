use std::{collections::HashMap, error::Error, fmt, str::FromStr};

#[derive(Clone)]
pub(crate) enum Domain {
    Full(String),
    Suffix(String),
    Keyword(String),
    Regex(regex::Regex),
}

impl FromStr for Domain {
    type Err = Box<dyn Error + Send + Sync>;

    fn from_str(v: &str) -> Result<Self, Self::Err> {
        match v.split_once(':') {
            Some((t, s)) => {
                if s.is_empty() {
                    return Err("missing domain".into());
                }
                match t {
                    "full" => Ok(Self::Full(s.to_string())),
                    "suffix" => Ok(Self::Suffix(s.trim_start_matches('.').to_string())),
                    "regex" => {
                        let s = match regex::Regex::from_str(s) {
                            Ok(v) => v,
                            Err(e) => {
                                return Err(format!("invalid regex: {}", e).into());
                            }
                        };
                        Ok(Self::Regex(s))
                    }
                    "keyword" => Ok(Self::Keyword(s.to_string())),
                    _ => return Err(format!("unknown domain type: {}", t).into()),
                }
            }
            None => {
                if v.is_empty() {
                    return Err("missing domain".into());
                }
                Ok(Self::Full(v.to_string()))
            }
        }
    }
}

impl serde::Serialize for Domain {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            Self::Full(s) => format!("full:{}", s),
            Self::Suffix(s) => format!("suffix:{}", s),
            Self::Regex(s) => format!("regex:{}", s),
            Self::Keyword(s) => format!("keyword:{}", s),
        };
        serializer.serialize_str(&s)
    }
}

impl<'de> serde::Deserialize<'de> for Domain {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DomainVisitor;

        impl<'de> serde::de::Visitor<'de> for DomainVisitor {
            type Value = Domain;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a domain type")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(DomainVisitor)
    }
}

pub(crate) struct DomainMatcher {
    suffix_set: Option<succinct::SuccinctSet>,
    keywords: Option<Vec<String>>,
    regexs: Option<Vec<regex::Regex>>,
}

impl DomainMatcher {
    pub(crate) fn new(domains: Vec<Domain>) -> Self {
        let mut suffix_set_keys = Vec::new();
        let mut suffix_key_map = HashMap::new();
        let mut keywords = Vec::new();
        let mut regexs = Vec::new();
        for domain in domains {
            match domain {
                Domain::Full(s) => {
                    if suffix_key_map.insert(s.clone(), ()).is_none() {
                        suffix_set_keys.push(Self::reverse_domain(s));
                    }
                }
                Domain::Suffix(s) => {
                    if suffix_key_map.insert(s.clone(), ()).is_none() {
                        if s.starts_with('.') {
                            suffix_set_keys.push(Self::reverse_domain_suffix(s));
                        } else {
                            suffix_set_keys.push(Self::reverse_domain(s.clone()));
                            suffix_set_keys.push(Self::reverse_root_domain_suffix(s));
                        }
                    }
                }
                Domain::Keyword(k) => {
                    keywords.push(k);
                }
                Domain::Regex(r) => {
                    regexs.push(r);
                }
            }
        }
        suffix_set_keys.sort();
        let suffix_set = if !suffix_set_keys.is_empty() {
            Some(succinct::SuccinctSet::new(suffix_set_keys))
        } else {
            None
        };
        let keywords = if keywords.len() > 0 {
            keywords.shrink_to_fit();
            Some(keywords)
        } else {
            None
        };
        let regexs = if regexs.len() > 0 {
            regexs.shrink_to_fit();
            Some(regexs)
        } else {
            None
        };
        Self {
            suffix_set,
            keywords,
            regexs,
        }
    }

    pub(crate) fn find(&self, domain: &str) -> bool {
        if let Some(suffix_set) = &self.suffix_set {
            if suffix_set.has(domain) {
                return true;
            }
        }
        if let Some(keywords) = &self.keywords {
            if keywords.iter().any(|k| domain.contains(k)) {
                return true;
            }
        }
        if let Some(regexs) = &self.regexs {
            if regexs.iter().any(|r| r.is_match(domain)) {
                return true;
            }
        }
        false
    }

    fn reverse_domain(domain: String) -> String {
        domain.chars().rev().collect()
    }

    fn reverse_domain_suffix(domain: String) -> String {
        let mut s = domain.chars().rev().collect::<String>();
        s.push(succinct::PREFIX_LABEL);
        s.shrink_to_fit();
        s
    }

    fn reverse_root_domain_suffix(domain: String) -> String {
        let mut s = domain.chars().rev().collect::<String>();
        s.push('.');
        s.push(succinct::PREFIX_LABEL);
        s.shrink_to_fit();
        s
    }
}

mod succinct {
    use std::collections::VecDeque;

    lazy_static::lazy_static! {
        static ref MARK: [u64; 65] = {
            let mut mark = [0u64; 65];
            for i in 0..65 {
                mark[i] = (1 << i) - 1;
            }
            mark
        };

        static ref RMARK: [u64; 64] = {
            let mut mark = [0u64; 64];
            let mut r_mark = [0u64; 64];
            for i in 0..64 {
                mark[i] = (1 << (i + 1)) - 1;
                r_mark[i] = !mark[i];
            }
            r_mark
        };

        static ref SELECT_8_LOOKUP: [u8; 2048] = {
            let mut select_8_lookup = [0u8; 2048];
            for i in 0u8..=255 {
                let mut w = i;
                for j in 0..8 {
                    let x = w.trailing_zeros() as u8;
                    w = w & (w - 1);
                    select_8_lookup[(i as usize) * 8 + j] = x;
                }
            }
            select_8_lookup
        };
    }

    pub(super) const PREFIX_LABEL: char = '\r';

    pub(super) struct SuccinctSet {
        leaves: Vec<u64>,
        label_bitmap: Vec<u64>,
        labels: Vec<u8>,
        ranks: Vec<i32>,
        selects: Vec<i32>,
    }

    impl SuccinctSet {
        pub(super) fn new(keys: Vec<String>) -> Self {
            let mut leaves = Vec::new();
            let mut labels = Vec::new();
            let mut label_bitmap = Vec::new();
            let mut l_idx = 0;
            let mut queue = VecDeque::new();
            queue.push_back((0, keys.len(), 0));
            let mut i = 0;
            while let Some(mut elt) = queue.pop_front() {
                if elt.2 == keys[elt.1].len() {
                    elt.0 += 1;
                    Self::set_bit(&mut leaves, i, 1);
                }
                let mut j = elt.0;
                while j < elt.1 {
                    let frm = j;
                    while j < elt.1 && keys[j].as_bytes()[elt.2] == keys[frm].as_bytes()[elt.2] {
                        j += 1;
                    }
                    queue.push_back((frm, j, elt.2 + 1));
                    labels.push(keys[frm].as_bytes()[elt.2]);
                    Self::set_bit(&mut label_bitmap, l_idx, 0);
                    l_idx += 1;
                }
                Self::set_bit(&mut label_bitmap, l_idx, 1);
                l_idx += 1;
                i += 1;
            }
            let (selects, ranks) = Self::index_select32r64(&mut label_bitmap);
            Self {
                leaves,
                label_bitmap,
                labels,
                ranks,
                selects,
            }
        }

        pub(super) fn has<S: AsRef<str>>(&self, key: S) -> bool {
            let mut node_id = 0;
            let mut bm_idx = 0;
            let key_slice = key.as_ref().as_bytes();
            for i in 0..key_slice.len() {
                let current_char = key_slice[i];
                loop {
                    if Self::get_bit(&self.label_bitmap, bm_idx) != 0 {
                        return false;
                    }
                    let next_label = self.labels[(bm_idx - node_id) as usize];
                    if next_label as char == PREFIX_LABEL {
                        return true;
                    }
                    if next_label == current_char {
                        break;
                    }
                    bm_idx += 1;
                }
                node_id = Self::count_zeros(&self.label_bitmap, &self.ranks, bm_idx + 1);
                bm_idx = Self::select_ith_one(
                    &self.label_bitmap,
                    &self.ranks,
                    &self.selects,
                    node_id - 1,
                ) + 1;
            }
            if Self::get_bit(&self.leaves, node_id) != 0 {
                return true;
            }
            loop {
                if Self::get_bit(&self.label_bitmap, bm_idx) != 0 {
                    return false;
                }
                if self.labels[(bm_idx - node_id) as usize] as char == PREFIX_LABEL {
                    return true;
                }
                bm_idx += 1;
            }
        }

        fn set_bit(bm: &mut Vec<u64>, i: isize, v: isize) {
            while (i >> 6) as usize >= bm.len() {
                bm.push(0);
            }
            bm[(i >> 6) as usize] |= (v as u64) << (i & 63);
        }

        fn index_select32r64(words: &mut Vec<u64>) -> (Vec<i32>, Vec<i32>) {
            let l = words.len() << 6;
            let mut sidx = Vec::with_capacity(words.len());
            let mut ith = -1;
            for i in 0..l {
                if words[i >> 6] & (1 << (i & 63)) != 0 {
                    ith += 1;
                    if ith & 31 == 0 {
                        sidx.push(i as i32);
                    }
                }
            }
            sidx.shrink_to_fit();
            (sidx, Self::index_rank64(words, Some(true)))
        }

        fn index_rank64(words: &mut Vec<u64>, opt: Option<bool>) -> Vec<i32> {
            let mut trailing = false;
            if let Some(o) = opt {
                trailing = o;
            }
            let mut l = words.len();
            if trailing {
                l += 1;
            }
            let mut idx = Vec::with_capacity(l);
            idx.resize(l, 0);
            let mut n = 0i32;
            for i in 0..words.len() {
                idx[i] = n;
                n += words[i].count_ones() as i32;
            }
            if trailing {
                idx[words.len()] = n;
            }
            idx
        }

        fn get_bit(bm: &Vec<u64>, i: isize) -> u64 {
            bm[(i >> 6) as usize] & (1 << (i & 63))
        }

        fn select32r64(
            words: &Vec<u64>,
            select_idx: &Vec<i32>,
            rank_idx: &Vec<i32>,
            i: i32,
        ) -> (i32, i32) {
            #[allow(unused_assignments)]
            let mut a = 0i32;
            let l = words.len() as i32;
            let mut word_i = select_idx[(i >> 5) as usize] >> 6;
            while rank_idx[(word_i + 1) as usize] <= i {
                word_i += 1;
            }
            let mut w = words[word_i as usize];
            let mut ww = w;
            let base = word_i << 6;
            let mut find_ith = (i - rank_idx[word_i as usize]) as isize;
            let mut offset = 0u32;
            let mut ones = (ww as u32).count_ones();
            if ones as isize <= find_ith {
                find_ith -= ones as isize;
                offset |= 32;
                ww >>= 32;
            }
            ones = (ww as u16).count_ones();
            if ones as isize <= find_ith {
                find_ith -= ones as isize;
                offset |= 16;
                ww >>= 16;
            }
            ones = (ww as u8).count_ones();
            if ones as isize <= find_ith {
                a = (SELECT_8_LOOKUP
                    [((ww >> 5) & (0x7f8) | (find_ith - ones as isize) as u64) as usize])
                    as i32
                    + offset as i32
                    + 8;
            } else {
                a = (SELECT_8_LOOKUP[((ww & 0xff) << 3 | (find_ith as u64)) as usize]) as i32
                    + offset as i32;
            }
            a += base;
            w &= RMARK[(a & 63) as usize];
            if w != 0 {
                return (a, base + ((w as u64).trailing_zeros() as i32));
            }
            word_i += 1;
            while word_i < l {
                w = words[word_i as usize];
                if w != 0 {
                    return (a, (word_i << 6) + ((w as u64).trailing_zeros() as i32));
                }
                word_i += 1;
            }
            (a, l << 6)
        }

        fn select_ith_one(bm: &Vec<u64>, ranks: &Vec<i32>, selects: &Vec<i32>, i: isize) -> isize {
            let (a, _) = Self::select32r64(bm, selects, ranks, i as i32);
            a as isize
        }

        fn rank64(words: &Vec<u64>, r_idx: &Vec<i32>, i: i32) -> (i32, i32) {
            let word_i = (i >> 6) as usize;
            let j = i & 63;
            let n = r_idx[word_i];
            let w = words[word_i];
            let c1 = n + (w & MARK[j as usize]).count_ones() as i32;
            (c1, ((w >> j) as i32) & 1)
        }

        fn count_zeros(bm: &Vec<u64>, ranks: &Vec<i32>, i: isize) -> isize {
            let (a, _) = Self::rank64(bm, ranks, i as i32);
            i - a as isize
        }
    }
}
