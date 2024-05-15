// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[inline]
fn unmask_fallback(payload: &mut [u8], mask: [u8; 4]) {
    for i in 0..payload.len() {
        payload[i] ^= mask[i & 3];
    }
}

#[inline]
fn unmask_fast(words: &mut [u32], mask: u32) {
    for word in words.iter_mut() {
        *word ^= mask;
    }
}

// Faster version of `unmask_easy()` which operates on 4-byte blocks.
// https://github.com/snapview/tungstenite-rs/blob/e5efe537b87a6705467043fe44bb220ddf7c1ce8/src/protocol/frame/mask.rs#L23
//
// https://rust.godbolt.org/z/qWq46ac8Y
pub fn unmask(buf: &mut [u8], mut mask: [u8; 4]) {
    let (prefix, words, suffix) = bytemuck::pod_align_to_mut::<u8, u32>(buf);
    let head = prefix.len() & 3;
    unmask_fallback(prefix, mask);
    mask.rotate_left(head);
    unmask_fallback(suffix, mask);

    unmask_fast(words, u32::from_ne_bytes(mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unmask() {
        let mut payload = [0u8; 33];
        let mask = [1, 2, 3, 4];
        unmask(&mut payload, mask);
        assert_eq!(
            &payload,
            &[
                1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4,
                1, 2, 3, 4, 1
            ]
        );
    }

    #[test]
    fn length_variation_unmask() {
        for len in &[0, 2, 3, 8, 16, 18, 31, 32, 40] {
            let mut payload = vec![0u8; *len];
            let mask = [1, 2, 3, 4];
            unmask(&mut payload, mask);

            let expected = (0..*len).map(|i| (i & 3) as u8 + 1).collect::<Vec<_>>();
            assert_eq!(payload, expected);
        }
    }

    #[test]
    fn length_variation_unmask_2() {
        for len in &[0, 2, 3, 8, 16, 18, 31, 32, 40] {
            let mut payload = vec![0u8; *len];
            let mask = rand::random::<[u8; 4]>();
            unmask(&mut payload, mask);

            let expected = (0..*len).map(|i| mask[i & 3]).collect::<Vec<_>>();
            assert_eq!(payload, expected);
        }
    }
}
