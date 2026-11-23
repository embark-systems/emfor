# Independent EMP2, EMP3 and EMP4 known-answer computation: RFC 5869
# HKDF-SHA256, RFC 8439 ChaCha20-Poly1305, HChaCha20 and XChaCha20-Poly1305
# from draft-irtf-cfrg-xchacha-03, and AES-256-GCM (FIPS 197, NIST SP 800-38D,
# checked against the test vectors of McGrew and Viega's GCM specification),
# in plain Python (hashlib/hmac only).
import hmac, hashlib, struct

def hkdf(salt, ikm, info, n):
    prk = hmac.new(salt, ikm, hashlib.sha256).digest()
    out, t, i = b"", b"", 1
    while len(out) < n:
        t = hmac.new(prk, t + info + bytes([i]), hashlib.sha256).digest()
        out += t; i += 1
    return out[:n]

def rotl(v, c): return ((v << c) & 0xffffffff) | (v >> (32 - c))
def qr(s, a, b, c, d):
    s[a]=(s[a]+s[b])&0xffffffff; s[d]^=s[a]; s[d]=rotl(s[d],16)
    s[c]=(s[c]+s[d])&0xffffffff; s[b]^=s[c]; s[b]=rotl(s[b],12)
    s[a]=(s[a]+s[b])&0xffffffff; s[d]^=s[a]; s[d]=rotl(s[d],8)
    s[c]=(s[c]+s[d])&0xffffffff; s[b]^=s[c]; s[b]=rotl(s[b],7)
def block(key, counter, nonce):
    st = [0x61707865,0x3320646e,0x79622d32,0x6b206574] + list(struct.unpack('<8I', key)) + [counter] + list(struct.unpack('<3I', nonce))
    w = st[:]
    for _ in range(10):
        qr(w,0,4,8,12); qr(w,1,5,9,13); qr(w,2,6,10,14); qr(w,3,7,11,15)
        qr(w,0,5,10,15); qr(w,1,6,11,12); qr(w,2,7,8,13); qr(w,3,4,9,14)
    return struct.pack('<16I', *[(a+b)&0xffffffff for a,b in zip(w,st)])
def chacha20(key, counter, nonce, data):
    out = bytearray()
    for i in range(0, len(data), 64):
        ks = block(key, counter + i//64, nonce)
        out += bytes(x^y for x,y in zip(data[i:i+64], ks))
    return bytes(out)
def poly1305(key, msg):
    r = int.from_bytes(key[:16],'little') & 0x0ffffffc0ffffffc0ffffffc0fffffff
    s = int.from_bytes(key[16:],'little'); p = (1<<130)-5; acc = 0
    for i in range(0, len(msg), 16):
        n = int.from_bytes(msg[i:i+16] + b'\x01', 'little')
        acc = ((acc + n) * r) % p
    return ((acc + s) & ((1<<128)-1)).to_bytes(16,'little')
def hchacha20(key, nonce16):
    # Draft section 2.2: the ChaCha20 state with the 16-byte nonce in the
    # counter and nonce words, 20 rounds, no addition of the input state;
    # the output is words 0-3 and 12-15.
    w = [0x61707865,0x3320646e,0x79622d32,0x6b206574] + list(struct.unpack('<8I', key)) + list(struct.unpack('<4I', nonce16))
    for _ in range(10):
        qr(w,0,4,8,12); qr(w,1,5,9,13); qr(w,2,6,10,14); qr(w,3,7,11,15)
        qr(w,0,5,10,15); qr(w,1,6,11,12); qr(w,2,7,8,13); qr(w,3,4,9,14)
    return struct.pack('<8I', *(w[0:4] + w[12:16]))
def pad16(b): return b'\x00' * ((16 - len(b) % 16) % 16)
def seal(key, nonce, pt, aad):
    otk = block(key, 0, nonce)[:32]
    ct = chacha20(key, 1, nonce, pt)
    mac = poly1305(otk, aad + pad16(aad) + ct + pad16(ct) + struct.pack('<QQ', len(aad), len(ct)))
    return ct + mac
def xseal(key, nonce24, pt, aad):
    # Draft section 2.3: subkey from the first 16 nonce bytes, then
    # ChaCha20-Poly1305 with 4 zero bytes and the last 8 nonce bytes.
    return seal(hchacha20(key, nonce24[:16]), b'\x00' * 4 + nonce24[16:], pt, aad)

def _aes_tables():
    # The S-box from its definition: inverse in GF(2^8), then the affine map.
    def mul(a, b):
        r = 0
        while b:
            if b & 1:
                r ^= a
            a = ((a << 1) ^ 0x11b) if a & 0x80 else a << 1
            b >>= 1
        return r
    inv = [0] * 256
    for a in range(1, 256):
        for b in range(1, 256):
            if mul(a, b) == 1:
                inv[a] = b
    sbox = []
    for a in range(256):
        x = inv[a]
        y = x
        for i in range(1, 5):
            y ^= ((x << i) | (x >> (8 - i))) & 0xff
        sbox.append(y ^ 0x63)
    return sbox, mul

SBOX, GMUL = _aes_tables()

def aes_expand(key):
    nk = len(key) // 4
    nr = nk + 6
    w = [list(key[4 * i:4 * i + 4]) for i in range(nk)]
    rcon = 1
    for i in range(nk, 4 * (nr + 1)):
        t = list(w[i - 1])
        if i % nk == 0:
            t = [SBOX[b] for b in t[1:] + t[:1]]
            t[0] ^= rcon
            rcon = GMUL(rcon, 2)
        elif nk > 6 and i % nk == 4:
            t = [SBOX[b] for b in t]
        w.append([a ^ b for a, b in zip(w[i - nk], t)])
    return [sum(w[4 * r:4 * r + 4], []) for r in range(nr + 1)]

def aes_encrypt_block(rk, block):
    st = [b ^ k for b, k in zip(block, rk[0])]
    for r in range(1, len(rk)):
        st = [SBOX[b] for b in st]
        st = [st[(i + 4 * (i % 4)) % 16] for i in range(16)]  # ShiftRows
        if r != len(rk) - 1:
            out = []
            for c in range(4):
                a = st[4 * c:4 * c + 4]
                out += [GMUL(a[0], 2) ^ GMUL(a[1], 3) ^ a[2] ^ a[3],
                        a[0] ^ GMUL(a[1], 2) ^ GMUL(a[2], 3) ^ a[3],
                        a[0] ^ a[1] ^ GMUL(a[2], 2) ^ GMUL(a[3], 3),
                        GMUL(a[0], 3) ^ a[1] ^ a[2] ^ GMUL(a[3], 2)]
            st = out
        st = [b ^ k for b, k in zip(st, rk[r])]
    return bytes(st)

def ghash_mul(x, y):
    # Multiplication in GF(2^128) with the GCM bit order (SP 800-38D 6.3).
    r = 0xe1 << 120
    z = 0
    v = y
    for i in range(128):
        if (x >> (127 - i)) & 1:
            z ^= v
        v = (v >> 1) ^ r if v & 1 else v >> 1
    return z

def ghash(h, aad, ct):
    def blocks(b):
        b = b + b'\x00' * ((16 - len(b) % 16) % 16)
        return [int.from_bytes(b[i:i + 16], 'big') for i in range(0, len(b), 16)]
    y = 0
    for blk in blocks(aad) + blocks(ct):
        y = ghash_mul(y ^ blk, h)
    return ghash_mul(y ^ ((len(aad) * 8) << 64 | len(ct) * 8), h)

def gcm_seal(key, iv12, pt, aad):
    rk = aes_expand(key)
    h = int.from_bytes(aes_encrypt_block(rk, bytes(16)), 'big')
    j0 = iv12 + b'\x00\x00\x00\x01'
    ct = b''
    for i in range(0, len(pt), 16):
        ctr = iv12 + struct.pack('>I', 2 + i // 16)
        ks = aes_encrypt_block(rk, ctr)
        ct += bytes(a ^ b for a, b in zip(pt[i:i + 16], ks))
    tag = int.from_bytes(aes_encrypt_block(rk, j0), 'big') ^ ghash(h, aad, ct)
    return ct + tag.to_bytes(16, 'big')

def main():
    # RFC 8439 section 2.8.2 test vector.
    k = bytes(range(0x80, 0xa0)); n = bytes.fromhex('070000004041424344454647')
    aad = bytes.fromhex('50515253c0c1c2c3c4c5c6c7')
    pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it."
    out = seal(k, n, pt, aad)
    assert out[-16:].hex() == '1ae10b594f09e26a7e902ecbd0600691', out[-16:].hex()
    assert out[:16].hex() == 'd31a8d34648e60db7b86afbc53ef7ec2'
    # RFC 5869 test case 1.
    okm = hkdf(bytes.fromhex('000102030405060708090a0b0c'), bytes.fromhex('0b'*22), bytes.fromhex('f0f1f2f3f4f5f6f7f8f9'), 42)
    assert okm.hex() == '3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865'
    # draft-irtf-cfrg-xchacha-03 section 2.2.1 (HChaCha20).
    sub = hchacha20(bytes(range(32)), bytes.fromhex('000000090000004a0000000031415927'))
    assert sub.hex() == '82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc', sub.hex()
    print("draft-irtf-cfrg-xchacha-03 2.2.1 HChaCha20: ok")
    # draft-irtf-cfrg-xchacha-03 appendix A.3.1 (AEAD_XChaCha20_Poly1305).
    xn = bytes.fromhex('404142434445464748494a4b4c4d4e4f5051525354555657')
    out = xseal(k, xn, pt, aad)
    want_ct = ('bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb'
               '731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452'
               '2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9'
               '21f9664c97637da9768812f615c68b13b52e')
    assert out[:-16].hex() == want_ct, out[:-16].hex()
    assert out[-16:].hex() == 'c0875924c1c7987947deafd8780acf49', out[-16:].hex()
    print("draft-irtf-cfrg-xchacha-03 A.3.1 AEAD_XChaCha20_Poly1305: ok")

    # FIPS 197 appendix C.3 (AES-256).
    rk = aes_expand(bytes(range(32)))
    assert aes_encrypt_block(rk, bytes.fromhex('00112233445566778899aabbccddeeff')).hex() == '8ea2b7ca516745bfeafc49904b496089'
    print("FIPS 197 C.3 AES-256: ok")
    # GCM specification (McGrew, Viega) test cases 15 and 16 (AES-256).
    gk = bytes.fromhex('feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308')
    giv = bytes.fromhex('cafebabefacedbaddecaf888')
    gp = bytes.fromhex('d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72'
                       '1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255')
    out = gcm_seal(gk, giv, gp, b'')
    assert out[:-16].hex() == ('522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa'
                               '8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662898015ad'), out[:-16].hex()
    assert out[-16:].hex() == 'b094dac5d93471bdec1a502270e3cc6c', out[-16:].hex()
    out = gcm_seal(gk, giv, gp[:60], bytes.fromhex('feedfacedeadbeeffeedfacedeadbeefabaddad2'))
    assert out[-16:].hex() == '76fc6ece0f4e1768cddf8853bb2d551b', out[-16:].hex()
    print("GCM specification test cases 15 and 16: ok")

    secret = bytes([7]*32)
    key = hkdf(b'emfor peers', secret, b'EMP2 chacha20-poly1305', 32)
    nonce = bytes(range(1, 13))
    payload = b'{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":1,"ts":1763000000000,"kind":"heartbeat"}'
    dgram = b'EMP2' + nonce + seal(key, nonce, payload, b'EMP2')
    print("key", key.hex())
    print("datagram", dgram.hex())
    print("len", len(dgram))

    key3 = hkdf(b'emfor peers', secret, b'EMP3 xchacha20-poly1305', 32)
    nonce3 = bytes(range(1, 25))
    dgram3 = b'EMP3' + nonce3 + xseal(key3, nonce3, payload, b'EMP3')
    print("EMP3 key", key3.hex())
    print("EMP3 datagram", dgram3.hex())
    print("EMP3 len", len(dgram3))

    # EMP4: key per sender start (info "EMP4 aes-256-gcm" + boot), nonce of
    # four zero bytes and the counter, AAD the 20 header bytes.
    boot = (1).to_bytes(8, 'big')
    counter = (5).to_bytes(8, 'big')
    key4 = hkdf(b'emfor peers', secret, b'EMP4 aes-256-gcm' + boot, 32)
    header = b'EMP4' + boot + counter
    dgram4 = header + gcm_seal(key4, bytes(4) + counter, payload, header)
    print("EMP4 key", key4.hex())
    print("EMP4 datagram", dgram4.hex())
    print("EMP4 len", len(dgram4))


if __name__ == "__main__":
    main()
