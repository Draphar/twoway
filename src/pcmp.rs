//! SSE4.2 (pcmpestri) accelerated substring search
//!
//! Using the two way substring search algorithm.
// wssm word size string matching<br>
// wslm word size lexicographical maximum suffix
//

#![allow(dead_code)]

extern crate unchecked_index;
extern crate memchr;

use std::cmp;
use std::iter::Zip;

use self::unchecked_index::get_unchecked;

use TwoWaySearcher;

fn zip<I, J>(i: I, j: J) -> Zip<I::IntoIter, J::IntoIter>
    where I: IntoIterator,
          J: IntoIterator
{
    i.into_iter().zip(j)
}

#[cfg(target_arch = "x86")]
use std::arch::x86::*;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// `pcmpestri`
///
/// “Packed compare explicit length strings (return index)”
///
/// PCMPESTRI xmm1, xmm2/m128, imm8
///
/// Return value: least index for start of (partial) match, (16 if no match).
#[target_feature(enable = "sse4.2")]
unsafe fn pcmpestri_16(text: *const u8, offset: usize, text_len: usize,
                       needle: __m128i, needle_len: usize) -> u32 {
    //debug_assert!(text_len + offset <= text.len()); // saturates at 16
    //debug_assert!(needle_len <= 16); // saturates at 16
    let text = _mm_loadu_si128(text.offset(offset as _) as *const _);
    _mm_cmpestri(needle, needle_len as _, text, text_len as _, _SIDD_CMP_EQUAL_ORDERED) as _
}

/// `pcmpestrm`
///
/// “Packed compare explicit length strings (return mask)”
///
/// PCMPESTRM xmm1, xmm2/m128, imm8
///
/// Return value: bitmask in the 16 lsb of the return value.
#[target_feature(enable = "sse4.2")]
unsafe fn pcmpestrm_eq_each(text: *const u8, offset: usize, text_len: usize,
                            needle: *const u8, noffset: usize, needle_len: usize) -> u64 {
    // NOTE: text *must* be readable for 16 bytes
    // NOTE: needle *must* be readable for 16 bytes
    //debug_assert!(text_len + offset <= text.len()); // saturates at 16
    //debug_assert!(needle_len <= 16); // saturates at 16
    let needle = _mm_loadu_si128(needle.offset(noffset as _) as *const _);
    let text = _mm_loadu_si128(text.offset(offset as _) as *const _);
    let mask = _mm_cmpestrm(needle, needle_len as _, text, text_len as _, _SIDD_CMP_EQUAL_EACH);

    #[cfg(target_arch = "x86")] {
        let mut res: u64 = ::std::mem::uninitialized();
        _mm_storel_epi64(&mut res, mask);
        res
    }

    #[cfg(target_arch = "x86_64")] {
        _mm_extract_epi64(mask, 0) as _
    }
}


/// Search for first possible match of `pat` -- might be just a byte
/// Return `(pos, length)` length of match
#[cfg(test)]
fn first_start_of_match(text: &[u8], pat: &[u8]) -> Option<(usize, usize)> {
    // not safe for text that is non aligned and ends at page boundary
    let patl = pat.len();
    assert!(patl <= 16);
    unsafe { first_start_of_match_inner(text, pat, pat128(pat)) }
}

/// Safe wrapper around pcmpestri to find first match of `pat` in `text`.
/// `p` contains the first two words of `pat` and *must* match.
/// Length given by length of `pat`, only first 16 bytes considered.
#[target_feature(enable = "sse4.2")]
unsafe fn first_start_of_match_inner(text: &[u8], pat: &[u8], p: __m128i) -> Option<(usize, usize)> {
    // align the text pointer
    let tp = text.as_ptr();
    let tp_align_offset = tp as usize & 0xF;
    let init_len;
    let tp_aligned;

    if tp_align_offset != 0 {
        init_len = 16 - tp_align_offset;
        tp_aligned = tp.offset(-(tp_align_offset as isize));
    } else {
        init_len = 0;
        tp_aligned = tp;
    }

    let patl = pat.len();
    debug_assert!(patl <= 16);

    let mut offset = 0;

    // search the unaligned prefix first
    if init_len > 0 {
        for start in 0..cmp::min(init_len, text.len()) {
            if text[start] != pat[0] {
                continue;
            }
            let mut mlen = 1;
            for (&a, &b) in zip(&text[start + 1..], &pat[1..]) {
                if a != b {
                    mlen = 0;
                    break;
                }
                mlen += 1;
            }
            return Some((start, mlen))
        }
        offset += 16;
    }
    while text.len() >= offset - tp_align_offset + patl {
        let tlen = text.len() - (offset - tp_align_offset);
        let ret = pcmpestri_16(tp_aligned, offset, tlen, p, patl) as usize;
        if ret == 16 {
            offset += 16;
        } else {
            let match_len = cmp::min(patl, 16 - ret);
            return Some((offset - tp_align_offset + ret, match_len));
        }
    }

    None
}

/// safe to search unaligned for first start of match
///
/// unsafe because the end of text must not be close (within 16 bytes) of a page boundary
unsafe fn first_start_of_match_unaligned(text: &[u8], pat_len: usize, p: __m128i) -> Option<(usize, usize)> {
    let tp = text.as_ptr();
    debug_assert!(pat_len <= 16);
    debug_assert!(pat_len <= text.len());

    let mut offset = 0;

    while text.len() - pat_len >= offset {
        let tlen = text.len() - offset;
        let ret = pcmpestri_16(tp, offset, tlen, p, pat_len) as usize;
        if ret == 16 {
            offset += 16;
        } else {
            let match_len = cmp::min(pat_len, 16 - ret);
            return Some((offset + ret, match_len));
        }
    }

    None
}

#[test]
fn test_first_start_of_match() {
    let text = b"abc";
    let longer = "longer text and so on";
    assert_eq!(first_start_of_match(text, b"d"), None);
    assert_eq!(first_start_of_match(text, b"c"), Some((2, 1)));
    assert_eq!(first_start_of_match(text, b"abc"), Some((0, 3)));
    assert_eq!(first_start_of_match(text, b"T"), None);
    assert_eq!(first_start_of_match(text, b"\0text"), None);
    assert_eq!(first_start_of_match(text, b"\0"), None);

    // test all windows
    for wsz in 1..17 {
        for window in longer.as_bytes().windows(wsz) {
            let str_find = longer.find(::std::str::from_utf8(window).unwrap());
            assert!(str_find.is_some());
            let first_start = first_start_of_match(longer.as_bytes(), window);
            assert!(first_start.is_some());
            let (pos, len) = first_start.unwrap();
            assert!(len <= wsz);
            assert!(len == wsz && Some(pos) == str_find
                    || pos <= str_find.unwrap());
        }
    }
}

fn find_2byte_pat(text: &[u8], pat: &[u8]) -> Option<(usize, usize)> {
    debug_assert!(text.len() >= pat.len());
    debug_assert!(pat.len() == 2);
    // Search for the second byte of the pattern, not the first, better for
    // scripts where we have two-byte encoded codepoints (the first byte will
    // repeat much more often than the second).
    let mut off = 1;
    while let Some(i) = memchr::memchr(pat[1], &text[off..]) {
        match text.get(off + i - 1) {
            None => break,
            Some(&c) if c == pat[0] => return Some((off + i - 1, off + i + 1)),
            _ => off += i + 1,
        }

    }
    None
}

/// Simd text search optimized for short patterns (<= 8 bytes)
#[target_feature(enable = "sse4.2")]
unsafe fn find_short_pat(text: &[u8], pat: &[u8]) -> Option<usize> {
    debug_assert!(pat.len() <= 8);
    /*
    if pat.len() == 2 {
        return find_2byte_pat(text, pat);
    }
    */
    let r = pat128(pat);

    // safe part of text -- everything but the last 16 bytes
    let safetext = &text[..cmp::max(text.len(), 16) - 16];

    let mut pos = 0;
    'search: loop {
        if pos + pat.len() > safetext.len() {
            break;
        }
        // find the next occurence
        match first_start_of_match_unaligned(&safetext[pos..], pat.len(), r) {
            None => break, // no matches
            Some((mpos, mlen)) => {
                pos += mpos;
                if mlen < pat.len() {
                    if pos > text.len() - pat.len() {
                        return None;
                    }
                    for (&a, &b) in zip(&text[pos + mlen..], &pat[mlen..]) {
                        if a != b {
                            pos += 1;
                            continue 'search;
                        }
                    }
                }

                return Some(pos);
            }
        }
    }

    'tail: loop {
        if pos > text.len() - pat.len() {
            return None;
        }
        // find the next occurence
        match first_start_of_match_inner(&text[pos..], pat, r) {
            None => return None, // no matches
            Some((mpos, mlen)) => {
                pos += mpos;
                if mlen < pat.len() {
                    if pos > text.len() - pat.len() {
                        return None;
                    }
                    for (&a, &b) in zip(&text[pos + mlen..], &pat[mlen..]) {
                        if a != b {
                            pos += 1;
                            continue 'tail;
                        }
                    }
                }

                return Some(pos);
            }
        }
    }
}

/// `is_supported` checks whether necessary SSE 4.2 feature is supported on current CPU.
pub fn is_supported() -> bool {
    if cfg!(feature = "use_std") {
        is_x86_feature_detected!("sse4.2")
    } else {
        cfg!(target_feature = "sse4.2")
    }
}

/// `find` finds the first ocurrence of `pattern` in the `text`.
///
/// This is the SSE42 accelerated version.
pub fn find(text: &[u8], pattern: &[u8]) -> Option<usize> {
    assert!(is_supported());

    if pattern.is_empty() {
        return Some(0);
    } else if text.len() < pattern.len() {
        return None;
    } else if pattern.len() == 1 {
        return memchr::memchr(pattern[0], text);
    } else {
        unsafe { find_inner(text, pattern) }
    }
}

#[target_feature(enable = "sse4.2")]
pub(crate) unsafe fn find_inner(text: &[u8], pat: &[u8]) -> Option<usize> {
    if pat.len() <= 6 {
        return find_short_pat(text, pat);
    }

    // real two way algorithm
    //

    // `memory` is the number of bytes of the left half that we already know
    let (crit_pos, mut period) = TwoWaySearcher::crit_params(pat);
    let mut memory;

    if &pat[..crit_pos] == &pat[period.. period + crit_pos] {
        memory = 0; // use memory
    } else {
        memory = !0; // !0 means memory is unused
        // approximation to the true period
        period = cmp::max(crit_pos, pat.len() - crit_pos) + 1;
    }

    //println!("pat: {:?}, crit={}, period={}", pat, crit_pos, period);
    let (left, right) = pat.split_at(crit_pos);
    let (right16, _right17) = right.split_at(cmp::min(16, right.len()));
    assert!(right.len() != 0);

    let r = pat128(right);

    // safe part of text -- everything but the last 16 bytes
    let safetext = &text[..cmp::max(text.len(), 16) - 16];

    let mut pos = 0;
    if memory == !0 {
        // Long period case -- no memory, period is an approximation
        'search: loop {
            if pos + pat.len() > safetext.len() {
                break;
            }
            // find the next occurence of the right half
            let start = crit_pos;
            match first_start_of_match_unaligned(&safetext[pos + start..], right16.len(), r) {
                None => break, // no matches
                Some((mpos, mlen)) => {
                    pos += mpos;
                    let mut pfxlen = mlen;
                    if pfxlen < right.len() {
                        pfxlen += shared_prefix_inner(&text[pos + start + mlen..], &right[mlen..]);
                    }
                    if pfxlen != right.len() {
                        // partial match
                        // skip by the number of bytes matched
                        pos += pfxlen + 1;
                        continue 'search;
                    } else {
                        // matches right part
                    }
                }
            }

            // See if the left part of the needle matches
            // XXX: Original algorithm compares from right to left here
            if left != &text[pos..pos + left.len()] {
                pos += period;
                continue 'search;
            }

            return Some(pos);
        }
    } else {
        // Short period case -- use memory, true period
        'search_memory: loop {
            if pos + pat.len() > safetext.len() {
                break;
            }
            // find the next occurence of the right half
            //println!("memory trace pos={}, memory={}", pos, memory);
            let mut pfxlen = if memory == 0 {
                let start = crit_pos;
                match first_start_of_match_unaligned(&safetext[pos + start..], right16.len(), r) {
                    None => break, // no matches
                    Some((mpos, mlen)) => {
                        pos += mpos;
                        mlen
                    }
                }
            } else {
                memory - crit_pos
            };
            if pfxlen < right.len() {
                pfxlen += shared_prefix_inner(&text[pos + crit_pos + pfxlen..], &right[pfxlen..]);
            }
            if pfxlen != right.len() {
                // partial match
                // skip by the number of bytes matched
                pos += pfxlen + 1;
                memory = 0;
                continue 'search_memory;
            } else {
                // matches right part
            }

            // See if the left part of the needle matches
            // XXX: Original algorithm compares from right to left here
            if memory <= left.len() && &left[memory..] != &text[pos + memory..pos + left.len()] {
                pos += period;
                memory = pat.len() - period;
                continue 'search_memory;
            }

            return Some(pos);
        }
    }

    // no memory used for final part
    'tail: loop {
        if pos > text.len() - pat.len() {
            return None;
        }
        // find the next occurence of the right half
        let start = crit_pos;
        match first_start_of_match_inner(&text[pos + start..], right16, r) {
            None => return None, // no matches
            Some((mpos, mlen)) => {
                pos += mpos;
                let mut pfxlen = mlen;
                if pfxlen < right.len() {
                    pfxlen += shared_prefix_inner(&text[pos + start + mlen..], &right[mlen..]);
                }
                if pfxlen != right.len() {
                    // partial match
                    // skip by the number of bytes matched
                    pos += pfxlen + 1;
                    continue 'tail;

                } else {
                    // matches right part
                }
            }
        }

        // See if the left part of the needle matches
        // XXX: Original algorithm compares from right to left here
        if left != &text[pos..pos + left.len()] {
            pos += period;
            continue 'tail;
        }

        return Some(pos);
    }
}

#[test]
fn test_find() {
    let text = b"abc";
    assert_eq!(find(text, b"d"), None);
    assert_eq!(find(text, b"c"), Some(2));

    let longer = "longer text and so on, a bit more";

    // test all windows
    for wsz in 1..longer.len() {
        for window in longer.as_bytes().windows(wsz) {
            let str_find = longer.find(::std::str::from_utf8(window).unwrap());
            assert!(str_find.is_some());
            assert_eq!(find(longer.as_bytes(), window), str_find);
        }
    }

    let pat = b"ger text and so on";
    assert!(pat.len() > 16);
    assert_eq!(Some(3), find(longer.as_bytes(), pat));

    // test short period case

    let text = "cbabababcbabababab";
    let n = "abababab";
    assert_eq!(text.find(n), find(text.as_bytes(), n.as_bytes()));

    // memoized case -- this is tricky
    let text = "cbababababababababababababababab";
    let n = "abababab";
    assert_eq!(text.find(n), find(text.as_bytes(), n.as_bytes()));

}

/// Load the first 16 bytes of `pat` into a SIMD vector.
#[inline(always)]
fn pat128(pat: &[u8]) -> __m128i {
    unsafe { _mm_loadu_si128(pat.as_ptr() as *const _) }
}

/// Find longest shared prefix, return its length
///
/// Alignment safe: works for any text, pat.
pub fn shared_prefix(text: &[u8], pat: &[u8]) -> usize {
    assert!(is_supported());

    unsafe { shared_prefix_inner(text, pat) }
}

#[target_feature(enable = "sse4.2")]
unsafe fn shared_prefix_inner(text: &[u8], pat: &[u8]) -> usize {
    let tp = text.as_ptr();
    let tlen = text.len();
    let pp = pat.as_ptr();
    let plen = pat.len();
    let len = cmp::min(tlen, plen);

    // TODO: do non-aligned prefix manually too(?) aligned text or pat..
    // all but the end we can process with pcmpestrm
    let initial_part = len.saturating_sub(16);
    let mut prefix_len = 0;
    let mut offset = 0;
    while offset < initial_part {
        let initial_tail = initial_part - offset;
        let mask = pcmpestrm_eq_each(tp, offset, initial_tail, pp, offset, initial_tail);
        // find zero in the first 16 bits
        if mask != 0xffff {
            let first_bit_set = (mask ^ 0xffff).trailing_zeros() as usize;
            prefix_len += first_bit_set;
            return prefix_len;
        } else {
            prefix_len += cmp::min(initial_tail, 16);
        }
        offset += 16;
    }
    // so one block left, the last (up to) 16 bytes
    // unchecked slicing .. we don't want panics in this function
    let text_suffix = get_unchecked(text, prefix_len..len);
    let pat_suffix = get_unchecked(pat, prefix_len..len);
    for (&a, &b) in zip(text_suffix, pat_suffix) {
        if a != b {
            break;
        }
        prefix_len += 1;
    }

    prefix_len
}

#[test]
fn test_prefixlen() {
    let text_long  = b"0123456789abcdefeffect";
    let text_long2 = b"9123456789abcdefeffect";
    let text_long3 = b"0123456789abcdefgffect";
    let plen = shared_prefix(text_long, text_long);
    assert_eq!(plen, text_long.len());
    let plen = shared_prefix(b"abcd", b"abc");
    assert_eq!(plen, 3);
    let plen = shared_prefix(b"abcd", b"abcf");
    assert_eq!(plen, 3);
    assert_eq!(0, shared_prefix(text_long, text_long2));
    assert_eq!(0, shared_prefix(text_long, &text_long[1..]));
    assert_eq!(16, shared_prefix(text_long, text_long3));

    for i in 0..text_long.len() + 1 {
        assert_eq!(text_long.len() - i, shared_prefix(&text_long[i..], &text_long[i..]));
    }

    let l1 = [7u8; 1024];
    let mut l2 = [7u8; 1024];
    let off = 1000;
    l2[off] = 0;
    for i in 0..off {
        let plen = shared_prefix(&l1[i..], &l2[i..]);
        assert_eq!(plen, off - i);
    }
}
