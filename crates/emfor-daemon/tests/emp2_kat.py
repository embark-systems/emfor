# Independent EMP2 and EMP3 known-answer computation: RFC 5869 HKDF-SHA256,
# RFC 8439 ChaCha20-Poly1305, and HChaCha20 and XChaCha20-Poly1305 from
# draft-irtf-cfrg-xchacha-03, in plain Python (hashlib/hmac only).
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


if __name__ == "__main__":
    main()
