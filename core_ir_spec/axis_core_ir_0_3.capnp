@0xb3c9d2a4f18e6c71;

# ============================================================================
# Axis Core IR 0.3 — Canonical Binary Schema
#
# Status: CANONICAL for Core IR version 0.3
# ============================================================================

struct CoreBundle {
  version        @0 :Text;
  coreTerm       @1 :CoreTerm;
  entrypointName @2 :Text;
  entrypointId   @3 :UInt64;
  annotations    @4 :List(Annotation);
}

struct CoreTerm {
  nodeId @0 :UInt64;
  span   @1 :Span;

  union {
    cIntLit  @2  :CIntLit;
    cBoolLit @3  :CBoolLit;
    cUnitLit @4  :CUnitLit;
    cLam     @5  :CLam;
    cLet     @6  :CLet;
    cIf      @7  :CIf;
    cVar     @8  :CVar;
    cApp     @9  :CApp;
    cCall    @10 :CCall;
  }
}

struct CIntLit  { value @0 :Int64; }
struct CBoolLit { value @0 :Bool; }
struct CUnitLit {}
struct CLam { param @0 :Text; body  @1 :CoreTerm; }
struct CLet { name  @0 :Text; value @1 :CoreTerm; body @2 :CoreTerm; }
struct CIf  { cond  @0 :CoreTerm; then @1 :CoreTerm; else @2 :CoreTerm; }
struct CVar { name  @0 :Text; }
struct CApp { fn    @0 :CoreTerm; arg @1 :CoreTerm; }
struct CCall { targetName @0 :Text; args @1 :List(CoreTerm); }

struct Annotation { id @0 :Text; kind @1 :Text; data @2 :Text; }
struct Span { file @0 :Text; start @1 :UInt32; end @2 :UInt32; }
