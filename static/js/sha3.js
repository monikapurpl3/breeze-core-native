// SHA3-512, because the browser does not have it.
//
// WebCrypto's digest() covers SHA-1 and the SHA-2 family and nothing else, and
// the v2 request signature is defined over a **SHA3-512 digest of the body** (see
// meow_ac/security/signing.py). The app gets it from pointycastle; the panel has
// no bundler and no dependencies, so here is a Keccak.
//
// Correctness is not taken on trust: tools/sha3-selftest.js runs this against
// the runtime's own native sha3-512 over the published vectors, every ASCII
// length that crosses the 72-byte rate boundary, and random inputs. A digest
// that is wrong by one byte would produce signatures the server rejects with
// `bad_signature` and no hint as to why.
//
// Lanes are BigInt. A 32-bit-pair implementation is faster, and irrelevant here:
// the bodies being hashed are a few hundred bytes of JSON, once per request.

const MASK64 = (1n << 64n) - 1n;

// Keccak-f[1600] round constants.
const RC = [
  0x0000000000000001n, 0x0000000000008082n, 0x800000000000808an, 0x8000000080008000n,
  0x000000000000808bn, 0x0000000080000001n, 0x8000000080008081n, 0x8000000000008009n,
  0x000000000000008an, 0x0000000000000088n, 0x0000000080008009n, 0x000000008000000an,
  0x000000008000808bn, 0x800000000000008bn, 0x8000000000008089n, 0x8000000000008003n,
  0x8000000000008002n, 0x8000000000000080n, 0x000000000000800an, 0x800000008000000an,
  0x8000000080008081n, 0x8000000000008080n, 0x0000000080000001n, 0x8000000080008008n,
];

// Rotation offsets, as the specification prints them: rows are x, columns are y.
// Flattened programmatically into lane order (x + 5y) rather than by hand -- the
// first attempt at this transposed the table, which produced a hash that was
// wrong for every single input including the empty string.
const R_TABLE = [
  [0, 36, 3, 41, 18],    // x = 0, y = 0..4
  [1, 44, 10, 45, 2],    // x = 1
  [62, 6, 43, 15, 61],   // x = 2
  [28, 55, 25, 21, 56],  // x = 3
  [27, 20, 39, 8, 14],   // x = 4
];
const ROT = new Array(25);
for (let x = 0; x < 5; x++) {
  for (let y = 0; y < 5; y++) ROT[x + 5 * y] = BigInt(R_TABLE[x][y]);
}

const rotl = (v, n) => n === 0n ? v : ((v << n) | (v >> (64n - n))) & MASK64;

function keccakF(A) {
  for (let round = 0; round < 24; round++) {
    // θ
    const C = new Array(5);
    for (let x = 0; x < 5; x++) C[x] = A[x] ^ A[x + 5] ^ A[x + 10] ^ A[x + 15] ^ A[x + 20];
    for (let x = 0; x < 5; x++) {
      const D = C[(x + 4) % 5] ^ rotl(C[(x + 1) % 5], 1n);
      for (let y = 0; y < 25; y += 5) A[x + y] ^= D;
    }
    // ρ and π together: (x,y) -> (y, 2x+3y)
    const B = new Array(25).fill(0n);
    for (let x = 0; x < 5; x++) {
      for (let y = 0; y < 5; y++) {
        B[y + 5 * ((2 * x + 3 * y) % 5)] = rotl(A[x + 5 * y], ROT[x + 5 * y]);
      }
    }
    // χ
    for (let y = 0; y < 25; y += 5) {
      for (let x = 0; x < 5; x++) {
        A[x + y] = B[x + y] ^ (~B[((x + 1) % 5) + y] & B[((x + 2) % 5) + y] & MASK64);
      }
    }
    // ι
    A[0] ^= RC[round];
  }
  return A;
}

/** SHA3-512 of a Uint8Array, as a lowercase hex string. */
export function sha3_512_hex(bytes) {
  const RATE = 72;              // 1600/8 - 2*512/8
  const A = new Array(25).fill(0n);

  // Pad: SHA-3 domain separation 0x06, zeros, high bit of the last rate byte.
  const padded = new Uint8Array(Math.ceil((bytes.length + 1) / RATE) * RATE);
  padded.set(bytes);
  padded[bytes.length] = 0x06;
  padded[padded.length - 1] |= 0x80;

  for (let off = 0; off < padded.length; off += RATE) {
    // Absorb the block, little-endian lanes.
    for (let i = 0; i < RATE / 8; i++) {
      let lane = 0n;
      for (let b = 7; b >= 0; b--) lane = (lane << 8n) | BigInt(padded[off + i * 8 + b]);
      A[i] ^= lane;
    }
    keccakF(A);
  }

  // Squeeze 64 bytes. They fit inside one rate block, so no second permutation.
  let out = "";
  for (let i = 0; i < 8; i++) {
    let lane = A[i];
    for (let b = 0; b < 8; b++) {
      out += (Number(lane & 0xffn)).toString(16).padStart(2, "0");
      lane >>= 8n;
    }
  }
  return out;
}

/** Convenience: SHA3-512 hex of a UTF-8 string. */
export const sha3_512_hex_utf8 = (str) =>
  sha3_512_hex(new TextEncoder().encode(str));
