//! Gzip (RFC 1952) around a DEFLATE (RFC 1951) compressor, for the wasm module embedded in the single-file
//! page (the browser unpacks it with `DecompressionStream("gzip")`). Written here because the build has to be
//! one command on every platform with nothing to install: the standard library has no compressor, and the
//! system's `gzip` is not on every machine.
//!
//! LZ77 with hash chains and one step of lazy matching, then dynamic Huffman codes per block. On the wasm
//! module it lands within a few percent of `gzip -9`.

const WINDOW: usize = 32768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;
/// Candidates tried per position: more finds longer matches and costs time.
const MAX_CHAIN: usize = 128;
/// A three-byte match further back than this costs more bits than three literals.
const TOO_FAR: usize = 4096;
/// Tokens per block: each block gets Huffman codes fitted to its own content.
const BLOCK_TOKENS: usize = 1 << 16;
const NO_POS: u32 = u32::MAX;

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order in which the lengths of the code-length code are stored.
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
const END_OF_BLOCK: usize = 256;

/// A literal byte (`len == 0`, the byte in `dist`) or a back-reference.
#[derive(Clone, Copy)]
struct Token {
    len: u16,
    dist: u16,
}

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    count: u32,
}

impl BitWriter {
    /// Append the low `count` bits of `value`, least significant first.
    fn bits(&mut self, value: u32, count: u32) {
        self.acc |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.count -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn hash(data: &[u8], i: usize) -> usize {
    let v = u32::from(data[i]) << 16 | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]);
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

struct Matcher<'a> {
    data: &'a [u8],
    /// Most recent position with each hash.
    head: Vec<u32>,
    /// For each position, the previous one with the same hash.
    prev: Vec<u32>,
}

impl Matcher<'_> {
    fn insert(&mut self, i: usize) {
        if i + MIN_MATCH <= self.data.len() {
            let h = hash(self.data, i);
            self.prev[i] = self.head[h];
            self.head[h] = i as u32;
        }
    }

    /// The longest earlier occurrence of what starts at `i`: `(length, distance)`, length 0 for none.
    /// Call before [`Self::insert`] of the same position.
    fn longest(&self, i: usize) -> (usize, usize) {
        let data = self.data;
        if i + MIN_MATCH > data.len() {
            return (0, 0);
        }
        let limit = MAX_MATCH.min(data.len() - i);
        let (mut best_len, mut best_dist) = (0, 0);
        let mut candidate = self.head[hash(data, i)];
        for _ in 0..MAX_CHAIN {
            if candidate == NO_POS {
                break;
            }
            let c = candidate as usize;
            let dist = i - c;
            if dist > WINDOW {
                break;
            }
            // Cheap rejection: a longer match must agree at the position that would extend the best one.
            if best_len == 0 || data[c + best_len] == data[i + best_len] {
                let len = data[c..c + limit]
                    .iter()
                    .zip(&data[i..i + limit])
                    .take_while(|(a, b)| a == b)
                    .count();
                if len > best_len {
                    (best_len, best_dist) = (len, dist);
                    if len == limit {
                        break;
                    }
                }
            }
            candidate = self.prev[c];
        }
        if best_len < MIN_MATCH || (best_len == MIN_MATCH && best_dist > TOO_FAR) {
            (0, 0)
        } else {
            (best_len, best_dist)
        }
    }
}

fn tokenize(data: &[u8]) -> Vec<Token> {
    let mut m = Matcher {
        data,
        head: vec![NO_POS; 1 << HASH_BITS],
        prev: vec![NO_POS; data.len()],
    };
    let mut tokens = Vec::with_capacity(data.len() / 3);
    let mut i = 0;
    while i < data.len() {
        let (len, dist) = m.longest(i);
        m.insert(i);
        // Lazy matching: if the next position has a longer match, this byte is better sent as a literal.
        let take = len >= MIN_MATCH && (len >= MAX_MATCH || m.longest(i + 1).0 <= len);
        if take {
            tokens.push(Token {
                len: len as u16,
                dist: dist as u16,
            });
            for j in i + 1..i + len {
                m.insert(j);
            }
            i += len;
        } else {
            tokens.push(Token {
                len: 0,
                dist: u16::from(data[i]),
            });
            i += 1;
        }
    }
    tokens
}

/// Huffman code lengths for the given symbol frequencies, none longer than `limit` bits.
fn code_lengths(freq: &[u32], limit: usize) -> Vec<u8> {
    let mut lens = vec![0u8; freq.len()];
    let mut used: Vec<usize> = (0..freq.len()).filter(|&s| freq[s] > 0).collect();
    match used.len() {
        0 => return lens,
        1 => {
            lens[used[0]] = 1;
            return lens;
        }
        _ => {}
    }
    // Plain Huffman: repeatedly join the two lightest nodes. `parent[n]` is the node `n` was joined into.
    let leaves = used.len();
    let mut weight: Vec<u64> = used.iter().map(|&s| u64::from(freq[s])).collect();
    let mut parent = vec![usize::MAX; leaves];
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u64, usize)>> = weight
        .iter()
        .enumerate()
        .map(|(n, &w)| std::cmp::Reverse((w, n)))
        .collect();
    while heap.len() > 1 {
        let std::cmp::Reverse((wa, a)) = heap.pop().expect("two nodes");
        let std::cmp::Reverse((wb, b)) = heap.pop().expect("two nodes");
        let node = weight.len();
        weight.push(wa + wb);
        parent.push(usize::MAX);
        parent[a] = node;
        parent[b] = node;
        heap.push(std::cmp::Reverse((wa + wb, node)));
    }
    // How many codes of each length; lengths over the limit are folded into it and the tree repaired so
    // the code stays complete (one code leaves the limit, one shorter code splits in two).
    let mut count = vec![0u32; limit + 1];
    for leaf in 0..leaves {
        let mut depth = 0;
        let mut n = leaf;
        while parent[n] != usize::MAX {
            n = parent[n];
            depth += 1;
        }
        count[depth.min(limit)] += 1;
    }
    let full = 1u64 << limit;
    let mut total: u64 = (1..=limit)
        .map(|len| u64::from(count[len]) << (limit - len))
        .sum();
    while total > full {
        count[limit] -= 1;
        for len in (1..limit).rev() {
            if count[len] > 0 {
                count[len] -= 1;
                count[len + 1] += 2;
                break;
            }
        }
        total -= 1;
    }
    // The most frequent symbols get the shortest codes.
    used.sort_by_key(|&s| (freq[s], s));
    let mut next = used.len();
    for (len, &n) in count.iter().enumerate().skip(1) {
        for _ in 0..n {
            next -= 1;
            lens[used[next]] = len as u8;
        }
    }
    lens
}

/// Canonical codes for the lengths, already bit-reversed for the least-significant-first bit stream.
fn codes(lens: &[u8]) -> Vec<u16> {
    let mut count = [0u16; 16];
    for &l in lens {
        count[usize::from(l)] += 1;
    }
    count[0] = 0;
    let mut next = [0u16; 16];
    for len in 1..16 {
        next[len] = (next[len - 1] + count[len - 1]) << 1;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let code = next[usize::from(l)];
            next[usize::from(l)] += 1;
            code.reverse_bits() >> (16 - u32::from(l))
        })
        .collect()
}

/// A decoder needs at least two codes in a table to tell them apart; give unused symbols a token weight.
fn at_least_two(freq: &mut [u32]) {
    let mut used = freq.iter().filter(|&&f| f > 0).count();
    for f in freq.iter_mut() {
        if used >= 2 {
            break;
        }
        if *f == 0 {
            *f = 1;
            used += 1;
        }
    }
}

/// Index of the last table entry not above `value`.
fn slot(base: &[u16], value: usize) -> usize {
    base.iter()
        .rposition(|&b| usize::from(b) <= value)
        .unwrap_or(0)
}

fn write_block(w: &mut BitWriter, tokens: &[Token], last: bool) {
    let mut lit_freq = [0u32; 286];
    let mut dist_freq = [0u32; 30];
    for t in tokens {
        if t.len == 0 {
            lit_freq[usize::from(t.dist)] += 1;
        } else {
            lit_freq[257 + slot(&LEN_BASE, usize::from(t.len))] += 1;
            dist_freq[slot(&DIST_BASE, usize::from(t.dist))] += 1;
        }
    }
    lit_freq[END_OF_BLOCK] += 1;
    at_least_two(&mut lit_freq);
    at_least_two(&mut dist_freq);
    let lit_lens = code_lengths(&lit_freq, 15);
    let dist_lens = code_lengths(&dist_freq, 15);
    let trimmed = |lens: &[u8], min: usize| {
        lens.iter()
            .rposition(|&l| l != 0)
            .map_or(min, |i| (i + 1).max(min))
    };
    let hlit = trimmed(&lit_lens, 257);
    let hdist = trimmed(&dist_lens, 1);

    // Both tables' lengths as one run-length coded sequence: (code-length symbol, extra bits value).
    let seq: Vec<u8> = lit_lens[..hlit]
        .iter()
        .chain(&dist_lens[..hdist])
        .copied()
        .collect();
    let mut rle: Vec<(u8, u8)> = Vec::new();
    let mut i = 0;
    while i < seq.len() {
        let v = seq[i];
        let run_len = seq[i..].iter().take_while(|&&x| x == v).count();
        let mut run = run_len;
        if v == 0 {
            while run >= 11 {
                let n = run.min(138);
                rle.push((18, (n - 11) as u8));
                run -= n;
            }
            if run >= 3 {
                rle.push((17, (run - 3) as u8));
                run = 0;
            }
        } else {
            rle.push((v, 0));
            run -= 1;
            while run >= 3 {
                let n = run.min(6);
                rle.push((16, (n - 3) as u8));
                run -= n;
            }
        }
        for _ in 0..run {
            rle.push((v, 0));
        }
        i += run_len;
    }
    let mut cl_freq = [0u32; 19];
    for &(sym, _) in &rle {
        cl_freq[usize::from(sym)] += 1;
    }
    at_least_two(&mut cl_freq);
    let cl_lens = code_lengths(&cl_freq, 7);
    let cl_codes = codes(&cl_lens);
    let hclen = CL_ORDER
        .iter()
        .rposition(|&s| cl_lens[s] != 0)
        .map_or(4, |i| (i + 1).max(4));

    w.bits(u32::from(last), 1);
    w.bits(2, 2); // dynamic Huffman codes
    w.bits((hlit - 257) as u32, 5);
    w.bits((hdist - 1) as u32, 5);
    w.bits((hclen - 4) as u32, 4);
    for &s in &CL_ORDER[..hclen] {
        w.bits(u32::from(cl_lens[s]), 3);
    }
    for &(sym, extra) in &rle {
        let s = usize::from(sym);
        w.bits(u32::from(cl_codes[s]), u32::from(cl_lens[s]));
        match sym {
            16 => w.bits(u32::from(extra), 2),
            17 => w.bits(u32::from(extra), 3),
            18 => w.bits(u32::from(extra), 7),
            _ => {}
        }
    }

    let lit_codes = codes(&lit_lens);
    let dist_codes = codes(&dist_lens);
    let put_lit = |w: &mut BitWriter, s: usize| {
        w.bits(u32::from(lit_codes[s]), u32::from(lit_lens[s]));
    };
    for t in tokens {
        if t.len == 0 {
            put_lit(w, usize::from(t.dist));
            continue;
        }
        let (len, dist) = (usize::from(t.len), usize::from(t.dist));
        let ls = slot(&LEN_BASE, len);
        put_lit(w, 257 + ls);
        w.bits(
            (len - usize::from(LEN_BASE[ls])) as u32,
            u32::from(LEN_EXTRA[ls]),
        );
        let ds = slot(&DIST_BASE, dist);
        w.bits(u32::from(dist_codes[ds]), u32::from(dist_lens[ds]));
        w.bits(
            (dist - usize::from(DIST_BASE[ds])) as u32,
            u32::from(DIST_EXTRA[ds]),
        );
    }
    put_lit(w, END_OF_BLOCK);
}

fn deflate(data: &[u8], out: Vec<u8>) -> Vec<u8> {
    let tokens = tokenize(data);
    let mut w = BitWriter {
        out,
        acc: 0,
        count: 0,
    };
    let blocks = tokens.len().div_ceil(BLOCK_TOKENS).max(1);
    for b in 0..blocks {
        let start = b * BLOCK_TOKENS;
        let end = (start + BLOCK_TOKENS).min(tokens.len());
        write_block(&mut w, &tokens[start..end], b + 1 == blocks);
    }
    w.finish()
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, entry) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *entry = c;
    }
    !data.iter().fold(!0u32, |crc, &b| {
        table[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8)
    })
}

/// `data` as a gzip file (no name, no timestamp: the same input always gives the same bytes).
pub fn compress(data: &[u8]) -> Vec<u8> {
    // Magic, method 8 (deflate), no flags, no time, no extra flags, OS unknown.
    let header = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut out = deflate(data, header);
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain inflater for dynamic-Huffman blocks, independent of the code above: reads the stream the
    /// way RFC 1951 describes it.
    struct Inflater<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
    }

    impl Inflater<'_> {
        fn bits(&mut self, count: u32) -> u32 {
            let mut v = 0;
            for i in 0..count {
                let byte = self.data[self.pos];
                v |= u32::from((byte >> self.bit) & 1) << i;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
            }
            v
        }

        /// Decode one symbol of the canonical code given by `lens`.
        fn symbol(&mut self, lens: &[u8]) -> usize {
            let mut count = [0i32; 16];
            for &l in lens {
                count[usize::from(l)] += 1;
            }
            let mut sorted: Vec<usize> = (0..lens.len()).filter(|&s| lens[s] != 0).collect();
            sorted.sort_by_key(|&s| (lens[s], s));
            let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
            for &n in &count[1..] {
                code |= self.bits(1) as i32;
                if code - n < first {
                    return sorted[(index + code - first) as usize];
                }
                index += n;
                first = (first + n) << 1;
                code <<= 1;
            }
            panic!("invalid code");
        }

        fn inflate(&mut self) -> Vec<u8> {
            let mut out = Vec::new();
            loop {
                let last = self.bits(1) == 1;
                assert_eq!(self.bits(2), 2, "dynamic block");
                let hlit = self.bits(5) as usize + 257;
                let hdist = self.bits(5) as usize + 1;
                let hclen = self.bits(4) as usize + 4;
                let mut cl = [0u8; 19];
                for &s in &CL_ORDER[..hclen] {
                    cl[s] = self.bits(3) as u8;
                }
                let mut lens: Vec<u8> = Vec::new();
                while lens.len() < hlit + hdist {
                    match self.symbol(&cl) {
                        16 => {
                            let prev = *lens.last().expect("repeat needs a previous length");
                            let n = 3 + self.bits(2) as usize;
                            lens.extend(std::iter::repeat_n(prev, n));
                        }
                        17 => {
                            let n = 3 + self.bits(3) as usize;
                            lens.extend(std::iter::repeat_n(0, n));
                        }
                        18 => {
                            let n = 11 + self.bits(7) as usize;
                            lens.extend(std::iter::repeat_n(0, n));
                        }
                        l => lens.push(l as u8),
                    }
                }
                assert_eq!(lens.len(), hlit + hdist);
                let (lit, dist) = lens.split_at(hlit);
                loop {
                    let s = self.symbol(lit);
                    if s < 256 {
                        out.push(s as u8);
                    } else if s == END_OF_BLOCK {
                        break;
                    } else {
                        let ls = s - 257;
                        let len = usize::from(LEN_BASE[ls])
                            + self.bits(u32::from(LEN_EXTRA[ls])) as usize;
                        let ds = self.symbol(dist);
                        let d = usize::from(DIST_BASE[ds])
                            + self.bits(u32::from(DIST_EXTRA[ds])) as usize;
                        assert!(d <= out.len(), "distance before the start");
                        for _ in 0..len {
                            out.push(out[out.len() - d]);
                        }
                    }
                }
                if last {
                    return out;
                }
            }
        }
    }

    fn gunzip(gz: &[u8]) -> Vec<u8> {
        assert_eq!(&gz[..4], &[0x1f, 0x8b, 8, 0]);
        let body = &gz[10..gz.len() - 8];
        let out = Inflater {
            data: body,
            pos: 0,
            bit: 0,
        }
        .inflate();
        let tail = &gz[gz.len() - 8..];
        assert_eq!(
            u32::from_le_bytes(tail[..4].try_into().unwrap()),
            crc32(&out)
        );
        assert_eq!(
            u32::from_le_bytes(tail[4..].try_into().unwrap()),
            out.len() as u32
        );
        out
    }

    /// Deterministic bytes: runs, repeats at many distances and noise, like a binary.
    fn sample(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state >> 8
        };
        let mut out: Vec<u8> = Vec::with_capacity(len);
        while out.len() < len {
            match next() % 4 {
                0 => {
                    let n = 1 + next() as usize % 40;
                    let b = next() as u8;
                    out.extend(std::iter::repeat_n(b, n));
                }
                1 if out.len() > 8 => {
                    let d = 1 + next() as usize % out.len().min(40_000);
                    let n = 3 + next() as usize % 300;
                    for _ in 0..n {
                        out.push(out[out.len() - d]);
                    }
                }
                _ => {
                    let n = 1 + next() as usize % 20;
                    for _ in 0..n {
                        out.push((next() % 64) as u8);
                    }
                }
            }
        }
        out.truncate(len);
        out
    }

    #[test]
    fn crc_matches_the_reference_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn round_trips_edge_cases() {
        let cases: [&[u8]; 7] = [
            b"",
            b"a",
            b"ab",
            b"aaa",
            b"abcabcabcabcabcabcabcabc",
            &[0u8; 100_000],
            &[7u8; 259],
        ];
        for case in cases {
            assert_eq!(gunzip(&compress(case)), case, "{} bytes", case.len());
        }
        // Every byte value, and data with no repeats at all.
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(gunzip(&compress(&all)), all);
    }

    #[test]
    fn round_trips_mixed_data_across_several_blocks() {
        for (len, seed) in [(1_000, 1), (70_000, 2), (600_000, 3)] {
            let data = sample(len, seed);
            let gz = compress(&data);
            assert_eq!(gunzip(&gz), data, "seed {seed}");
            assert!(
                gz.len() < data.len(),
                "seed {seed}: {} -> {}",
                data.len(),
                gz.len()
            );
        }
    }

    #[test]
    fn repetitive_data_shrinks_a_lot_and_output_is_deterministic() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(2000);
        let gz = compress(&data);
        assert!(gz.len() * 50 < data.len(), "{} -> {}", data.len(), gz.len());
        assert_eq!(gz, compress(&data));
        assert_eq!(gunzip(&gz), data);
    }

    #[test]
    fn code_lengths_respect_the_limit_and_stay_decodable() {
        // Fibonacci-like weights make a plain Huffman tree as deep as it has symbols.
        let mut freq = vec![0u32; 40];
        let (mut a, mut b) = (1u32, 1u32);
        for f in freq.iter_mut() {
            *f = a;
            (a, b) = (b, a.saturating_add(b));
        }
        for limit in [7, 15] {
            let lens = code_lengths(&freq, limit);
            assert!(lens.iter().all(|&l| l >= 1 && usize::from(l) <= limit));
            // Kraft sum of exactly one: a complete prefix code.
            let kraft: u64 = lens.iter().map(|&l| 1u64 << (limit - usize::from(l))).sum();
            assert_eq!(kraft, 1 << limit);
            // More frequent symbols never get longer codes.
            assert!(lens.windows(2).all(|w| w[0] >= w[1]));
        }
    }
}
