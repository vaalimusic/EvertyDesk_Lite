// Minimal ap_int/ap_uint stub for C-simulation without Vitis HLS toolchain.
// Supports widths 1–256 bit; backed by uint64_t (<=64) or byte array (>64).
#pragma once
#include <cstdint>
#include <cstring>
#include <queue>

// ── Narrow types (≤64 bit) ───────────────────────────────────────────────────

template<int W, typename Enable = void>
struct ap_uint {
    uint64_t v = 0;
    ap_uint() = default;
    explicit ap_uint(uint64_t x) : v(x & mask()) {}
    ap_uint(uint64_t x, int) : v(x) {}   // internal
    operator uint64_t() const { return v; }
    operator uint8_t()  const { return (uint8_t)v; }
    operator bool()     const { return v != 0; }
    bool operator==(ap_uint o) const { return v == o.v; }
    bool operator!=(ap_uint o) const { return v != o.v; }
    bool operator==(uint64_t x) const { return v == x; }
    bool operator!=(uint64_t x) const { return v != x; }
    ap_uint& operator=(uint64_t x) { v = x & mask(); return *this; }
    ap_uint& operator|=(uint64_t x) { v |= x & mask(); return *this; }
    ap_uint& operator^=(uint64_t x) { v ^= x; v &= mask(); return *this; }
    ap_uint operator^(ap_uint o) const { return ap_uint(v ^ o.v, 0); }
    ap_uint operator^(uint64_t x) const { return ap_uint(v ^ x, 0); }
    // range read: bits [hi:lo]
    uint64_t operator()(int hi, int lo) const {
        int bits = hi - lo + 1;
        uint64_t m = (bits >= 64) ? ~0ULL : ((1ULL << bits) - 1ULL);
        return (v >> lo) & m;
    }
private:
    static uint64_t mask() { return W >= 64 ? ~0ULL : ((1ULL << W) - 1ULL); }
};

// Wide types (>64 bit) — byte-array backing, only operators needed for csim
template<int W>
struct ap_uint<W, typename std::enable_if<(W > 64)>::type> {
    static constexpr int BYTES = (W + 7) / 8;
    uint8_t v[BYTES] = {};
    ap_uint() { memset(v, 0, BYTES); }
    ap_uint(uint64_t x) { memset(v, 0, BYTES); memcpy(v, &x, 8); }
    operator uint64_t() const { uint64_t r=0; memcpy(&r,v,8); return r; }
    bool operator==(const ap_uint& o) const { return memcmp(v,o.v,BYTES)==0; }
    bool operator!=(const ap_uint& o) const { return !(*this==o); }
    ap_uint& operator=(uint64_t x) { memset(v,0,BYTES); memcpy(v,&x,8); return *this; }
    ap_uint operator^(const ap_uint& o) const {
        ap_uint r; for(int i=0;i<BYTES;i++) r.v[i]=v[i]^o.v[i]; return r;
    }
    uint64_t operator()(int hi, int lo) const {
        // byte-level extraction, fits in uint64_t
        int bits = hi - lo + 1; int byte0 = lo/8;
        uint64_t r=0; memcpy(&r, v+byte0, std::min(8,(BYTES-byte0)));
        r >>= (lo % 8);
        if (bits < 64) r &= (1ULL<<bits)-1ULL;
        return r;
    }
};

// ── Signed narrow type ───────────────────────────────────────────────────────

template<int W>
struct ap_int {
    int64_t v = 0;
    ap_int() = default;
    ap_int(int64_t x) : v(x) {}
    operator int64_t() const { return v; }
    operator int8_t()  const { return (int8_t)v; }
    ap_int operator-() const { return ap_int(-v); }
    bool operator==(ap_int o) const { return v == o.v; }
    bool operator==(int64_t x) const { return v == x; }
    bool operator!=(int64_t x) const { return v != x; }
    ap_int& operator=(int64_t x) { v = x; return *this; }
};

// Free operator^ for ap_uint
template<int W>
inline ap_uint<W> operator^(uint64_t a, ap_uint<W> b) { return b ^ a; }

// Disable HLS pragmas on non-Vitis compilers
#ifndef __SYNTHESIS__
  #ifdef _MSC_VER
    #define HLS_PRAGMA_IGNORE
  #endif
#endif
