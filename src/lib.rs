//! The 64-bit variant of xxHash.
//!
//! This is a *very* fast hash, running circles around all major hashing
//! algorithms, capable of saturating the *memory bandwidth* of older
//! hardware. The implementation has been extensively micro-optimized to
//! take full advantage of the instruction-level parallelism of modern
//! hardware.
//!
//! Do not use this for cryptography.
//!
//! https://code.google.com/p/xxhash/



// ILP is the heart of this algorithm. The optimized version of xxh32 gains
// over 2x from `-C no-vectorize-slp`. This means `unsafe`.
//
// It's probably best to consider this code as an excercise in writing
// good tests.

#![crate_name="xxhash"]
#![crate_type="lib"]

#![allow(unused_assignments, unused_variables)] // `read_ptr!`


// unstable
//#[cfg(test)]
// extern crate test;

use std::mem::{uninitialized, transmute};
use std::ptr::{copy};
use std::hash::{Hash, Hasher};
use std::default::Default;
use std::num::Wrapping;
use std::ops::{Shl, Shr, BitOr};

//unstable
//#[cfg(test)] use test::Bencher;

pub mod macros;
pub mod xxh32;

// large prime, new_with_seed(0) is so boring
const HAPPY_SEED: u64 = 18446744073709551557_u64;

const PRIME1: Wrapping<u64> =     Wrapping(11400714785074694791_u64);
const PRIME2: Wrapping<u64> =     Wrapping(14029467366897019727_u64);
const PRIME3: Wrapping<u64> =     Wrapping( 1609587929392839161_u64);
const PRIME4: Wrapping<u64> =     Wrapping( 9650029242287828579_u64);
const PRIME5: Wrapping<u64> =     Wrapping( 2870177450012600261_u64);

fn rotl64<T: Shl<usize, Output=T> + Shr<usize, Output=T> + BitOr<T, Output=T> + Clone>(x: T, b: usize) -> T { #![inline(always)]
    (x.clone() << b) | (x.clone() >> (64 - b))
}

pub fn oneshot(input: &[u8], seed: u64) -> u64 { #![inline]
    let mut state = XXHasher::new_with_seed(seed);
    state.write(input);
    state.finish()
}

#[derive(Copy)]
pub struct XXHasher {
    memory: [u64; 4],
    v1: u64,
    v2: u64,
    v3: u64,
    v4: u64,
    total_len: u64,
    seed: u64,
    memsize: usize,
}

impl XXHasher {
    /// Unless testing, randomize the seed for each set of
    /// hashes, e.g. when creating a new `HashMap`.
    pub fn new_with_seed(seed: u64) -> XXHasher { #![inline]
        let mut state: XXHasher = unsafe { uninitialized() };
        state.seed = seed;
        state.reset();
        state
    }

    pub fn new() -> XXHasher { #![inline]
        XXHasher::new_with_seed(HAPPY_SEED)
    }

    /// Reinitialize. The next input will start a new hash.
    fn reset(&mut self) { #![inline]
        self.v1 = (Wrapping(self.seed) + PRIME1 + PRIME2).0;
        self.v2 = (Wrapping(self.seed) + PRIME2).0;
        self.v3 = self.seed;
        self.v4 = (Wrapping(self.seed) - PRIME1).0;
        self.total_len = 0;
        self.memsize = 0;
    }
}

impl Hasher for XXHasher {

    /// Compute the hash. This can be used for intermediate values too.
    fn finish(&self) -> u64 { #![inline] unsafe {
        let mut rem = self.memsize;
        let mut h64: Wrapping<u64> = if self.total_len < 32 {
            Wrapping(self.seed) + PRIME5
        } else {
            // we have saved state
            let mut v1: Wrapping<u64> = Wrapping(self.v1);
            let mut v2: Wrapping<u64> = Wrapping(self.v2);
            let mut v3: Wrapping<u64> = Wrapping(self.v3);
            let mut v4: Wrapping<u64> = Wrapping(self.v4);

            let mut h = rotl64(v1, 1) + rotl64(v2, 7) + rotl64(v3, 12) + rotl64(v4, 18);

            macro_rules! permute(($v: ident) => ({
                $v = $v * PRIME2; $v = rotl64($v, 31); $v = $v * PRIME1; h = h ^ $v; h = h * PRIME1 + PRIME4;
            }));
            // this step does not exist in xxh32
            permute!(v1); permute!(v2); permute!(v3); permute!(v4);

            h
        };

        // and now we eat all the remaining bytes.
        let mut p: *const u8 = transmute(&self.memory);
        macro_rules! read(($size:ty) => (Wrapping(read_ptr!(p, rem, $size) as u64)));

        h64 = h64 + Wrapping(self.total_len as u64);

        while rem >= 8 {
            let mut k1: Wrapping<u64> = read!(u64) * PRIME2; k1 = rotl64(k1, 31); k1 = k1 * PRIME1;
            h64 = h64 ^ k1;
            h64 = rotl64(h64, 27) * PRIME1 + PRIME4;
        }

        if rem >= 4 {
            h64 = h64 ^ (read!(u32) * PRIME1);
            h64 = rotl64(h64, 23) * PRIME2 + PRIME3;
        }

        while rem > 0 {
            h64 = h64 ^ (read!(u8) * PRIME5);
            h64 = rotl64(h64, 11) * PRIME1;
        }

        h64 = h64.clone() ^ (h64.clone() >> 33);
        h64 = h64 * PRIME2;
        h64 = h64.clone() ^ (h64.clone() >> 29);
        h64 = h64 * PRIME3;
        h64 = h64.clone() ^ (h64.clone() >> 32);

        h64.0
    }}

    /// This is where you feed your data in.
    fn write(&mut self, input: &[u8]) { unsafe {
        let mem: *mut u8 = transmute(&self.memory);
        let mut rem: usize = input.len();
        let mut data: *const u8 = input.as_ptr();

        self.total_len += rem as u64;

        // not enough data for one 32-byte chunk,
        // so just fill the buffer and return.
        if self.memsize + rem < 32 {
            let dst: *mut u8 = mem.offset(self.memsize as isize);
            copy(data, dst, rem);
            self.memsize += rem;
            return;
        }

        // some data left from previous update
        // fill the buffer and eat it
        if self.memsize != 0 {
            let dst: *mut u8 = mem.offset(self.memsize as isize);
            let bump: usize = 32 - self.memsize;
            copy(data, dst, bump);

            // `read_ptr!` target
            let mut p: *const u8 = transmute(mem);
            let mut r = 32;

            macro_rules! read(() => (Wrapping(read_ptr!(p, r, u64))));

            macro_rules! eat(($v: ident) => ({
                $v = $v + read!() * PRIME2; $v = rotl64($v, 31); $v = $v * PRIME1;
            }));

            // Detaching these does good things to performance.
            // LLVM is not quite smart enough to do it on its own.
            let mut v1: Wrapping<u64> = Wrapping(self.v1);
            let mut v2: Wrapping<u64> = Wrapping(self.v2);
            let mut v3: Wrapping<u64> = Wrapping(self.v3);
            let mut v4: Wrapping<u64> = Wrapping(self.v4);

            eat!(v1); eat!(v2); eat!(v3); eat!(v4);

            // save the state
            self.v1 = v1.0;
            self.v2 = v2.0;
            self.v3 = v3.0;
            self.v4 = v4.0;

            data = data.offset(bump as isize);
            rem -= bump;
            self.memsize = 0;
        }

        {
            macro_rules! read(() => (Wrapping(read_ptr!(data, rem, u64))));

            // Note how `$v` does not depend on any other `v` in this phase.
            // This is critical for speed.
            macro_rules! eat(($v: ident) => ({
                $v = $v + read!() * PRIME2; $v = rotl64($v, 31); $v = $v * PRIME1;
            }));

            // again, go faster stripes
            let mut v1: Wrapping<u64> = Wrapping(self.v1);
            let mut v2: Wrapping<u64> = Wrapping(self.v2);
            let mut v3: Wrapping<u64> = Wrapping(self.v3);
            let mut v4: Wrapping<u64> = Wrapping(self.v4);

            // the main loop: eat whole chunks
            while rem >= 32 {
                eat!(v1); eat!(v2); eat!(v3); eat!(v4);
            }

            self.v1 = v1.0;
            self.v2 = v2.0;
            self.v3 = v3.0;
            self.v4 = v4.0;
        }

        // we have data left, so save it
        if rem > 0 {
            copy(data, mem, rem);
            self.memsize = rem;
        }
    }}

}

impl Clone for XXHasher {
    fn clone(&self) -> XXHasher { #![inline]
        *self
    }
}

impl Default for XXHasher {
    fn default() -> XXHasher { #![inline]
        XXHasher::new()
    }
}

pub fn hash<T: ?Sized + Hash>(value: &T) -> u64
{
    let mut state = XXHasher::new();
    value.hash(&mut state);
    state.finish()
}

pub fn hash_with_seed<T: ?Sized + Hash>(seed: u64, value: &T) -> u64 { #![inline]
    let mut state = XXHasher::new_with_seed(seed);
    value.hash(&mut state);
    state.finish()
}

/// the official sanity test
#[cfg(test)]
fn test_base<F>(f: F) where F: Fn(&[u8], u64) -> u64 {
    static BUFSIZE: usize = 101;
    static PRIME: u32 = 2654435761;

    let mut random: Wrapping<u32> = Wrapping(PRIME);
    let mut buf: Vec<u8> = Vec::with_capacity(BUFSIZE);
    for _ in 0..BUFSIZE {
        buf.push((random.0 >> 24) as u8);
        random = random * random;
    }

    let test = |size: usize, seed: u64, expected: u64| {
        let result = f(buf.split_at(size).0, seed);
        assert_eq!(result, expected);
    };

    test(1,                0,             0x4FCE394CC88952D8);
    test(1,                PRIME as u64,  0x739840CB819FA723);
    test(14,               0,             0xCFFA8DB881BC3A3D);
    test(14,               PRIME as u64,  0x5B9611585EFCC9CB);
    test(BUFSIZE,          0,             0x0EAB543384F878AD);
    test(BUFSIZE,          PRIME as u64,  0xCAA65939306F1E21);
}

//unstable
/*#[cfg(test)]
#[inline(always)]
fn bench_base<F>(bench: &mut Bencher, f: F )
    where F: Fn(&[u8]) -> u64
{
    static BUFSIZE: usize = 64*1024;

    let mut v: Vec<u8> = Vec::with_capacity(BUFSIZE);
    for i in 0..BUFSIZE {
        v.push(i as u8);
    }

    bench.iter( || f(v.as_slice()) );
    bench.bytes = BUFSIZE as u64;
}*/

#[test]
fn test_oneshot() {
    test_base(|v, seed|{
        let mut state = XXHasher::new_with_seed(seed);
        state.write(v);
        state.finish()
    })
}

#[test]
fn test_chunks() {
    test_base(|v, seed|{
        let mut state = XXHasher::new_with_seed(seed);
        for chunk in v.chunks(15) {
            state.write(chunk);
        }
        state.finish()
    })
}

// unstable
/*#[bench]
fn bench_64k_oneshot(b: &mut Bencher) {
    bench_base(b, |v| oneshot(v, 0))
}*/

/*
    * The following tests match those of SipHash.
    */


#[test] #[cfg(target_arch = "arm")]
fn test_hash_usize() {
    let val = 0xdeadbeef_deadbeef_u64;
    assert!(hash(&(val as u64)) != hash(&(val as usize)));
    assert_eq!(hash(&(val as u32)), hash(&(val as usize)));
}
#[test] #[cfg(target_arch = "x86_64")]
fn test_hash_usize() {
    let val = 0xdeadbeef_deadbeef_u64;
    assert_eq!(hash(&(val as u64)), hash(&(val as usize)));
    assert!(hash(&(val as u32)) != hash(&(val as usize)));
}
#[test] #[cfg(target_arch = "x86")]
fn test_hash_usize() {
    let val = 0xdeadbeef_deadbeef_u64;
    assert!(hash(&(val as u64)) != hash(&(val as usize)));
    assert_eq!(hash(&(val as u32)), hash(&(val as usize)));
}

#[test]
fn test_hash_idempotent() {
    let val64 = 0xdeadbeef_deadbeef_u64;
    assert_eq!(hash(&val64), hash(&val64));
    let val32 = 0xdeadbeef_u32;
    assert_eq!(hash(&val32), hash(&val32));
}

#[test]
fn test_hash_no_bytes_dropped_64() {
    let val = 0xdeadbeef_deadbeef_u64;

    assert!(hash(&val) != hash(&zero_byte(val, 0)));
    assert!(hash(&val) != hash(&zero_byte(val, 1)));
    assert!(hash(&val) != hash(&zero_byte(val, 2)));
    assert!(hash(&val) != hash(&zero_byte(val, 3)));
    assert!(hash(&val) != hash(&zero_byte(val, 4)));
    assert!(hash(&val) != hash(&zero_byte(val, 5)));
    assert!(hash(&val) != hash(&zero_byte(val, 6)));
    assert!(hash(&val) != hash(&zero_byte(val, 7)));

    fn zero_byte(val: u64, byte: usize) -> u64 {
        assert!(byte < 8);
        val & !(0xff << (byte * 8))
    }
}

#[test]
fn test_hash_no_bytes_dropped_32() {
    let val = 0xdeadbeef_u32;

    assert!(hash(&val) != hash(&zero_byte(val, 0)));
    assert!(hash(&val) != hash(&zero_byte(val, 1)));
    assert!(hash(&val) != hash(&zero_byte(val, 2)));
    assert!(hash(&val) != hash(&zero_byte(val, 3)));

    fn zero_byte(val: u32, byte: usize) -> u32 {
        assert!(byte < 4);
        val & !(0xff << (byte * 8))
    }
}

#[test]
fn test_hash_no_concat_alias() {
    let s = ("aa", "bb");
    let t = ("aabb", "");
    let u = ("a", "abb");

    assert!(s != t && t != u);
    assert!(hash(&s) != hash(&t) && hash(&s) != hash(&u));

    let a = [1u8, 0, 0, 0];
    let v: (&[u8], &[u8], &[u8]) = (&a[0..1], &a[1..3], &a[1..2]);
    let w: (&[u8], &[u8], &[u8]) = (&a[..], &a[0..0], &a[0..0]);

    assert!(v != w);
    assert!(hash(&v) != hash(&w));
}

// unstable
/*#[bench]
fn bench_str_under_8_bytes(b: &mut Bencher) {
    let s = "foo";
    b.bytes=s.len() as u64;
    b.iter(|| {
        hash(&s)
    })
}

#[bench]
fn bench_str_of_8_bytes(b: &mut Bencher) {
    let s = "foobar78";
    b.bytes = s.len() as u64;
    b.iter(|| {
        hash(&s);
    })
}

#[bench]
fn bench_str_over_8_bytes(b: &mut Bencher) {
    let s = "foobarbaz0";
    b.bytes = s.len() as u64;
    b.iter(|| {
        hash(&s)
    })
}

#[bench]
fn bench_long_str(b: &mut Bencher) {
    let s = "Lorem ipsum dolor sit amet, consectetur adipisicing elit, sed do eiusmod tempor \
incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud \
exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute \
irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla \
pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui \
officia deserunt mollit anim id est laborum.";
    b.bytes = s.len() as u64;
    b.iter(|| {
        hash(&s)
    })
}

#[bench]
fn bench_u64(b: &mut Bencher) {
    let u = 16262950014981195938u64;
    b.bytes = 8;
    b.iter(|| {
        hash(&u)
    })
}*/



