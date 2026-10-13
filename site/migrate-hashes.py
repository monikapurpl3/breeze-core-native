"""The ten hashes wiki/Upgrading-to-4.md lists for migrate.sh -- the maintainer's
list, which must follow every change to site/migrate.sh: CRC32, MD2, MD4, MD5, SHA1,
SHA256, SHA512, RipeMD128, RipeMD160, ED2K. MD2/MD4/RIPEMD-160 come from
pycryptodome; RIPEMD-128 is written out here (nothing at hand has it). ED2K is
MD4 for a file under one 9,728,000-byte chunk.

    python3 site/migrate-hashes.py <published migrate.sh> site/migrate.sh

Needs pycryptodome (Debian: python3-pycryptodome). Check that the published
file reproduces the wiki's current list before trusting the new one. Not
published: publish.sh uploads an allow-list."""
import hashlib
import struct
import sys
import zlib

try:
    from Crypto.Hash import MD2, MD4, RIPEMD160
except ImportError:
    from Cryptodome.Hash import MD2, MD4, RIPEMD160


def _rol(x, n):
    x &= 0xFFFFFFFF
    return ((x << n) | (x >> (32 - n))) & 0xFFFFFFFF


def ripemd128(data: bytes) -> str:
    h0, h1, h2, h3 = 0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476
    msg = data + b'\x80'
    msg += b'\x00' * ((56 - len(msg) % 64) % 64)
    msg += struct.pack('<Q', (len(data) * 8) & 0xFFFFFFFFFFFFFFFF)
    R = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
         7, 4, 13, 1, 10, 6, 15, 3, 12, 0, 9, 5, 2, 14, 11, 8,
         3, 10, 14, 4, 9, 15, 8, 1, 2, 7, 0, 6, 13, 11, 5, 12,
         1, 9, 11, 10, 0, 8, 12, 4, 13, 3, 7, 15, 14, 5, 6, 2]
    RP = [5, 14, 7, 0, 9, 2, 11, 4, 13, 6, 15, 8, 1, 10, 3, 12,
          6, 11, 3, 7, 0, 13, 5, 10, 14, 15, 8, 12, 4, 9, 1, 2,
          15, 5, 1, 3, 7, 14, 6, 9, 11, 8, 12, 2, 10, 0, 4, 13,
          8, 6, 4, 1, 3, 11, 15, 0, 5, 12, 2, 13, 9, 7, 10, 14]
    S = [11, 14, 15, 12, 5, 8, 7, 9, 11, 13, 14, 15, 6, 7, 9, 8,
         7, 6, 8, 13, 11, 9, 7, 15, 7, 12, 15, 9, 11, 7, 13, 12,
         11, 13, 6, 7, 14, 9, 13, 15, 14, 8, 13, 6, 5, 12, 7, 5,
         11, 12, 14, 15, 14, 15, 9, 8, 9, 14, 5, 6, 8, 6, 5, 12]
    SP = [8, 9, 9, 11, 13, 15, 15, 5, 7, 7, 8, 11, 14, 14, 12, 6,
          9, 13, 15, 7, 12, 8, 9, 11, 7, 7, 12, 7, 6, 15, 13, 11,
          9, 7, 15, 11, 8, 6, 6, 14, 12, 13, 5, 14, 13, 13, 7, 5,
          15, 5, 8, 11, 14, 14, 6, 14, 6, 9, 12, 9, 12, 5, 15, 8]
    K = [0x00000000, 0x5A827999, 0x6ED9EBA1, 0x8F1BBCDC]
    KP = [0x50A28BE6, 0x5C4DD124, 0x6D703EF3, 0x00000000]

    def f(j, x, y, z):
        if j < 16:
            return x ^ y ^ z
        if j < 32:
            return (x & y) | (~x & z)
        if j < 48:
            return (x | ~y) ^ z
        return (x & z) | (y & ~z)

    for off in range(0, len(msg), 64):
        X = struct.unpack('<16I', msg[off:off + 64])
        a, b, c, d = h0, h1, h2, h3
        ap, bp, cp, dp = h0, h1, h2, h3
        for j in range(64):
            t = _rol(a + (f(j, b, c, d) & 0xFFFFFFFF) + X[R[j]] + K[j // 16], S[j])
            a, d, c, b = d, c, b, t
            t = _rol(ap + (f(63 - j, bp, cp, dp) & 0xFFFFFFFF) + X[RP[j]] + KP[j // 16], SP[j])
            ap, dp, cp, bp = dp, cp, bp, t
        t = (h1 + c + dp) & 0xFFFFFFFF
        h1 = (h2 + d + ap) & 0xFFFFFFFF
        h2 = (h3 + a + bp) & 0xFFFFFFFF
        h3 = (h0 + b + cp) & 0xFFFFFFFF
        h0 = t
    return struct.pack('<4I', h0, h1, h2, h3).hex()


def tenhash(data: bytes) -> list[tuple[str, str]]:
    md4 = MD4.new(data).hexdigest()
    assert len(data) < 9728000, 'ED2K is MD4 only for a single chunk'
    return [
        ('CRC32', format(zlib.crc32(data) & 0xFFFFFFFF, '08x')),
        ('MD2', MD2.new(data).hexdigest()),
        ('MD4', md4),
        ('MD5', hashlib.md5(data).hexdigest()),
        ('SHA1', hashlib.sha1(data).hexdigest()),
        ('SHA256', hashlib.sha256(data).hexdigest()),
        ('SHA512', hashlib.sha512(data).hexdigest()),
        ('RipeMD128', ripemd128(data)),
        ('RipeMD160', RIPEMD160.new(data).hexdigest()),
        ('ED2K', md4),
    ]


if __name__ == '__main__':
    # Published test vectors first: RIPEMD-128 is the hand-written one.
    assert ripemd128(b'') == 'cdf26213a150dc3ecb610f18f6b38b46'
    assert ripemd128(b'abc') == 'c14a12199c66e4ba84636b0f69144c77'
    assert ripemd128(b'a' * 1000000) == '4a7f5723f954eba1216c9d8f6320431f'
    for path in sys.argv[1:]:
        print(f'== {path}')
        for name, value in tenhash(open(path, 'rb').read()):
            print(f'**{name}**: {value}')
