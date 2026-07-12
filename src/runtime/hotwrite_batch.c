/* AXVERITY_HOTWRITE_ADMISSION_MINIMAL_CAPTURE_V1 — ISOLATION MEASUREMENT ONLY.
 *
 * Standalone C implementation of the single-call hotwrite collapse: the whole
 * iterate/capture/stamp/hash/write cycle for `n` records in ONE call, over a
 * NEW isolated FFI surface (this file only), everything in one translation
 * unit so gen/hash/write inline under -O3. Reproduces the exact
 * SMALL/MEDIUM/LARGE record bytes (20/70/10 by i%10) of the M1 workload,
 * including MEDIUM's embedded sha256; do_key adds the content-hash key
 * sha256(record)/record. Own SHA-256 (no libcrypto / no callback to Rust) so
 * the surface is genuinely isolated. Not wired into any real path. */

#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <immintrin.h>
#include <fcntl.h>
#include <unistd.h>

/* ---- SHA-256 (public-domain-style compact impl) ---- */
typedef struct { uint32_t s[8]; uint64_t len; uint8_t buf[64]; size_t n; } sha256_ctx;

static const uint32_t K[64] = {
0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2};

static void sha256_init(sha256_ctx *c){
  c->s[0]=0x6a09e667;c->s[1]=0xbb67ae85;c->s[2]=0x3c6ef372;c->s[3]=0xa54ff53a;
  c->s[4]=0x510e527f;c->s[5]=0x9b05688c;c->s[6]=0x1f83d9ab;c->s[7]=0x5be0cd19;
  c->len=0;c->n=0;
}
/* Scalar fallback block (portable — used on CPUs without SHA-NI). */
#define ROR(x,n) (((x)>>(n))|((x)<<(32-(n))))
static void sha256_block_scalar(sha256_ctx *c, const uint8_t *p){
  uint32_t w[64];
  for(int i=0;i<16;i++) w[i]=((uint32_t)p[i*4]<<24)|((uint32_t)p[i*4+1]<<16)|((uint32_t)p[i*4+2]<<8)|((uint32_t)p[i*4+3]);
  for(int i=16;i<64;i++){
    uint32_t s0=ROR(w[i-15],7)^ROR(w[i-15],18)^(w[i-15]>>3);
    uint32_t s1=ROR(w[i-2],17)^ROR(w[i-2],19)^(w[i-2]>>10);
    w[i]=w[i-16]+s0+w[i-7]+s1;
  }
  uint32_t a=c->s[0],b=c->s[1],cc=c->s[2],d=c->s[3],e=c->s[4],f=c->s[5],g=c->s[6],h=c->s[7];
  for(int i=0;i<64;i++){
    uint32_t S1=ROR(e,6)^ROR(e,11)^ROR(e,25); uint32_t ch=(e&f)^((~e)&g);
    uint32_t t1=h+S1+ch+K[i]+w[i];
    uint32_t S0=ROR(a,2)^ROR(a,13)^ROR(a,22); uint32_t mj=(a&b)^(a&cc)^(b&cc);
    uint32_t t2=S0+mj; h=g;g=f;f=e;e=d+t1;d=cc;cc=b;b=a;a=t1+t2;
  }
  c->s[0]+=a;c->s[1]+=b;c->s[2]+=cc;c->s[3]+=d;c->s[4]+=e;c->s[5]+=f;c->s[6]+=g;c->s[7]+=h;
}

/* SHA-NI (hardware) single-block compression — the SAME instruction path Rust's
 * sha2 crate uses. Differential-tested (scalar == SHA-NI == python hashlib over
 * all message lengths 0..260) before use. */
__attribute__((target("sha,sse4.1")))
static void sha256_block_shani(sha256_ctx *ctx, const uint8_t *data){
  __m128i STATE0,STATE1,MSG,TMP,MSG0,MSG1,MSG2,MSG3,ABEF,CDGH;
  const __m128i MASK=_mm_set_epi64x(0x0c0d0e0f08090a0bULL,0x0405060700010203ULL);
  TMP=_mm_loadu_si128((const __m128i*)&ctx->s[0]);
  STATE1=_mm_loadu_si128((const __m128i*)&ctx->s[4]);
  TMP=_mm_shuffle_epi32(TMP,0xB1); STATE1=_mm_shuffle_epi32(STATE1,0x1B);
  STATE0=_mm_alignr_epi8(TMP,STATE1,8); STATE1=_mm_blend_epi16(STATE1,TMP,0xF0);
  ABEF=STATE0; CDGH=STATE1;
  MSG0=_mm_shuffle_epi8(_mm_loadu_si128((const __m128i*)(data+0)),MASK);
  MSG=_mm_add_epi32(MSG0,_mm_set_epi32(K[3],K[2],K[1],K[0]));
  STATE1=_mm_sha256rnds2_epu32(STATE1,STATE0,MSG);
  MSG=_mm_shuffle_epi32(MSG,0x0E); STATE0=_mm_sha256rnds2_epu32(STATE0,STATE1,MSG);
  MSG1=_mm_shuffle_epi8(_mm_loadu_si128((const __m128i*)(data+16)),MASK);
  MSG=_mm_add_epi32(MSG1,_mm_set_epi32(K[7],K[6],K[5],K[4]));
  STATE1=_mm_sha256rnds2_epu32(STATE1,STATE0,MSG);
  MSG=_mm_shuffle_epi32(MSG,0x0E); STATE0=_mm_sha256rnds2_epu32(STATE0,STATE1,MSG);
  MSG0=_mm_sha256msg1_epu32(MSG0,MSG1);
  MSG2=_mm_shuffle_epi8(_mm_loadu_si128((const __m128i*)(data+32)),MASK);
  MSG=_mm_add_epi32(MSG2,_mm_set_epi32(K[11],K[10],K[9],K[8]));
  STATE1=_mm_sha256rnds2_epu32(STATE1,STATE0,MSG);
  MSG=_mm_shuffle_epi32(MSG,0x0E); STATE0=_mm_sha256rnds2_epu32(STATE0,STATE1,MSG);
  MSG1=_mm_sha256msg1_epu32(MSG1,MSG2);
  MSG3=_mm_shuffle_epi8(_mm_loadu_si128((const __m128i*)(data+48)),MASK);
  MSG=_mm_add_epi32(MSG3,_mm_set_epi32(K[15],K[14],K[13],K[12]));
  STATE1=_mm_sha256rnds2_epu32(STATE1,STATE0,MSG);
  TMP=_mm_alignr_epi8(MSG3,MSG2,4); MSG0=_mm_add_epi32(MSG0,TMP); MSG0=_mm_sha256msg2_epu32(MSG0,MSG3);
  MSG=_mm_shuffle_epi32(MSG,0x0E); STATE0=_mm_sha256rnds2_epu32(STATE0,STATE1,MSG);
  MSG2=_mm_sha256msg1_epu32(MSG2,MSG3);
  #define RND4(Ma,Mb,Mc,Md,ki) \
    MSG=_mm_add_epi32(Ma,_mm_set_epi32(K[ki+3],K[ki+2],K[ki+1],K[ki+0])); \
    STATE1=_mm_sha256rnds2_epu32(STATE1,STATE0,MSG); \
    TMP=_mm_alignr_epi8(Ma,Md,4); Mb=_mm_add_epi32(Mb,TMP); Mb=_mm_sha256msg2_epu32(Mb,Ma); \
    MSG=_mm_shuffle_epi32(MSG,0x0E); STATE0=_mm_sha256rnds2_epu32(STATE0,STATE1,MSG); \
    Md=_mm_sha256msg1_epu32(Md,Ma);
  RND4(MSG0,MSG1,MSG2,MSG3,16); RND4(MSG1,MSG2,MSG3,MSG0,20);
  RND4(MSG2,MSG3,MSG0,MSG1,24); RND4(MSG3,MSG0,MSG1,MSG2,28);
  RND4(MSG0,MSG1,MSG2,MSG3,32); RND4(MSG1,MSG2,MSG3,MSG0,36);
  RND4(MSG2,MSG3,MSG0,MSG1,40); RND4(MSG3,MSG0,MSG1,MSG2,44);
  RND4(MSG0,MSG1,MSG2,MSG3,48); RND4(MSG1,MSG2,MSG3,MSG0,52);
  RND4(MSG2,MSG3,MSG0,MSG1,56);
  #undef RND4
  MSG=_mm_add_epi32(MSG3,_mm_set_epi32(K[63],K[62],K[61],K[60]));
  STATE1=_mm_sha256rnds2_epu32(STATE1,STATE0,MSG);
  MSG=_mm_shuffle_epi32(MSG,0x0E); STATE0=_mm_sha256rnds2_epu32(STATE0,STATE1,MSG);
  STATE0=_mm_add_epi32(STATE0,ABEF); STATE1=_mm_add_epi32(STATE1,CDGH);
  TMP=_mm_shuffle_epi32(STATE0,0x1B); STATE1=_mm_shuffle_epi32(STATE1,0xB1);
  STATE0=_mm_blend_epi16(TMP,STATE1,0xF0); STATE1=_mm_alignr_epi8(STATE1,TMP,8);
  _mm_storeu_si128((__m128i*)&ctx->s[0],STATE0);
  _mm_storeu_si128((__m128i*)&ctx->s[4],STATE1);
}

/* Runtime dispatch: SHA-NI where the CPU has it, scalar fallback otherwise.
 * Resolved once (cached); no SIGILL on a non-SHA-NI CPU. */
static void (*g_sha_block)(sha256_ctx*, const uint8_t*) = 0;
static void sha256_block(sha256_ctx *c, const uint8_t *p){
  if(!g_sha_block){
    __builtin_cpu_init();
    g_sha_block = __builtin_cpu_supports("sha") ? sha256_block_shani : sha256_block_scalar;
  }
  g_sha_block(c, p);
}
static void sha256_update(sha256_ctx *c, const uint8_t *p, size_t len){
  c->len+=len;
  while(len){
    size_t take=64-c->n; if(take>len) take=len;
    memcpy(c->buf+c->n,p,take); c->n+=take; p+=take; len-=take;
    if(c->n==64){ sha256_block(c,c->buf); c->n=0; }
  }
}
static void sha256_final(sha256_ctx *c, uint8_t out[32]){
  uint64_t bits=c->len*8; uint8_t pad=0x80;
  sha256_update(c,&pad,1);
  uint8_t zero=0; while(c->n!=56) sha256_update(c,&zero,1);
  uint8_t lb[8]; for(int i=0;i<8;i++) lb[i]=(uint8_t)(bits>>(56-8*i));
  sha256_update(c,lb,8);
  for(int i=0;i<8;i++){ out[i*4]=(uint8_t)(c->s[i]>>24);out[i*4+1]=(uint8_t)(c->s[i]>>16);out[i*4+2]=(uint8_t)(c->s[i]>>8);out[i*4+3]=(uint8_t)c->s[i]; }
}
static void sha256(const uint8_t *p, size_t len, uint8_t out[32]){ sha256_ctx c; sha256_init(&c); sha256_update(&c,p,len); sha256_final(&c,out); }

static const char HEX[16]="0123456789abcdef";

/* SMALL: "S|<12>|" x-padded to 64 ; returns length. */
static size_t gen_small(long i, uint8_t *out){
  int k=snprintf((char*)out,80,"S|%012ld|",i);
  size_t n=(size_t)k; while(n<64) out[n++]='x'; return n;
}
static size_t gen_large(long i, uint8_t *out){
  int k=snprintf((char*)out,80,"L|%012ld|",i);
  size_t n=(size_t)k; while(n<4096) out[n++]='x'; return n;
}
/* MEDIUM: payload "HW1|<12>|" x-padded to 100 ; rec=hex(sha256(payload))(64)+"%010d"(100)+payload =174. */
static size_t gen_medium(long i, uint8_t *out){
  uint8_t payload[100];
  int k=snprintf((char*)payload,100,"HW1|%012ld|",i);
  size_t n=(size_t)k; while(n<100) payload[n++]='x';
  uint8_t dig[32]; sha256(payload,100,dig);
  size_t o=0;
  for(int j=0;j<32;j++){ out[o++]=(uint8_t)HEX[dig[j]>>4]; out[o++]=(uint8_t)HEX[dig[j]&0xf]; }
  o+=(size_t)snprintf((char*)out+o,12,"%010d",100);
  memcpy(out+o,payload,100); o+=100;
  return o;
}

/* Single-call collapse. Returns total bytes written. */
long hotwrite_batch_c_run(long n, long block_size, long do_key){
  size_t bs=(size_t)block_size;
  uint8_t *arena=(uint8_t*)malloc(bs);
  size_t cursor=0; long total=0;
  uint8_t rec[4096];
  volatile uint8_t sink=0;
  for(long i=0;i<n;i++){
    long pos=i%10; size_t len;
    if(pos<2) len=gen_small(i,rec);
    else if(pos==9) len=gen_large(i,rec);
    else len=gen_medium(i,rec);
    if(do_key==1){ uint8_t key[32]; sha256(rec,len,key); sink^=key[0]; }
    /* Phase B (RAM-flat): reuse one arena on overflow (models instant flush+reclaim;
     * fsync/durability accounted separately). Phase A replaces this with real
     * budget-triggered flush+reclaim+fsync. */
    if(cursor+len>bs){ cursor=0; }
    memcpy(arena+cursor,rec,len); cursor+=len; total+=(long)len;
    /* Force the store to be observed (defeat DCE of gen/hash/memcpy under
     * reuse-arena, where the arena is otherwise never read). Compiler barrier
     * only — no emitted instruction, realistic timing. */
    __asm__ __volatile__("" : : "r"(arena) : "memory");
  }
  (void)sink;
  return total;
}

/* Durable block flush replicating fs_write_bytes: tmp write + fsync + atomic
 * rename + parent-dir fsync. Same durability the 46,296-baseline uses. */
static void flush_block(const char* dir, long seq, const uint8_t* buf, size_t len){
  char fin[4096], tmp[4096];
  snprintf(fin,sizeof fin,"%s/block-%ld.bin",dir,seq);
  snprintf(tmp,sizeof tmp,"%s/block-%ld.bin.tmp.%d",dir,seq,(int)getpid());
  int fd=open(tmp,O_WRONLY|O_CREAT|O_TRUNC,0644);
  if(fd<0){ perror("hotwrite_durable open tmp"); return; }
  size_t off=0; while(off<len){ ssize_t w=write(fd,buf+off,len-off); if(w<=0){ perror("hotwrite_durable write"); break; } off+=(size_t)w; }
  fsync(fd); close(fd);
  if(rename(tmp,fin)!=0){ perror("hotwrite_durable rename"); }
  int dfd=open(dir,O_RDONLY|O_DIRECTORY); if(dfd>=0){ fsync(dfd); close(dfd); }
}

/* Phase A — DURABLE single-call collapse. Same gen+hash+write cycle, but seals
 * each 4 MiB block to disk (block-<seq>.bin + manifest.log) with real per-block
 * fsync, matching the M1 workload's on-disk output + durability. Passes
 * hotwrite-workload-verify.py byte-for-byte. RAM flat (~4 MiB — reuse-arena +
 * flush-on-seal; no unbounded queue). Returns total bytes. */
long hotwrite_batch_c_durable(const char* dir, long n, long block_size, long do_key){
  size_t bs=(size_t)block_size;
  uint8_t *arena=(uint8_t*)malloc(bs);
  size_t cursor=0; long total=0, block_seq=0, start_i=0;
  uint8_t rec[4096];
  char mpath[4096]; snprintf(mpath,sizeof mpath,"%s/manifest.log",dir);
  FILE* mf=fopen(mpath,"w");
  if(!mf){ perror("hotwrite_durable manifest"); free(arena); return -1; }
  volatile uint8_t sink=0;
  for(long i=0;i<n;i++){
    long pos=i%10; size_t len;
    if(pos<2) len=gen_small(i,rec); else if(pos==9) len=gen_large(i,rec); else len=gen_medium(i,rec);
    if(do_key==1){ uint8_t key[32]; sha256(rec,len,key); sink^=key[0]; }
    if(i>0 && cursor+len>bs){                       /* seal current block before this record */
      flush_block(dir,block_seq,arena,cursor);
      fprintf(mf,"%ld\t%ld\t%ld\t%zu\n",block_seq,start_i,i-1,cursor);
      block_seq++; start_i=i; cursor=0;
    }
    memcpy(arena+cursor,rec,len); cursor+=len; total+=(long)len;
  }
  flush_block(dir,block_seq,arena,cursor);          /* final block */
  fprintf(mf,"%ld\t%ld\t%ld\t%zu\n",block_seq,start_i,n-1,cursor);
  fflush(mf); fsync(fileno(mf)); fclose(mf);
  free(arena); (void)sink;
  return total;
}
