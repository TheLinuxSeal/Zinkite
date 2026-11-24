"""The benchmarks `run.py` measures.

The first sixteen, with their sizes and their Goose, C++ and Rust implementations,
are Goose's own suite (github.com/aardappel/goose, bench/run_bench.py; see README.md),
with a Zinkite implementation of each added. The rest are classic benchmarks written
for this suite in all four languages.

Each entry: `name`; `what` it does; `param`, what the size is called (N if omitted);
`sizes`, the small, medium and large values of the `// BENCH_N` line; and per
language a list of implementations, each a row in the results. A C++ row is built
with -DVARIANT=<variant>, and its `tier` is idiomatic or expert.
"""

BENCHMARKS = [
    dict(
        name='sum',
        what='Control: build a flat i32 array, then scan it eight times.',
        sizes=[1000000, 16000000, 128000000],
        zinkite=[dict(label='zinkite grow-only array', file='sum.zn')],
        goose=[
            dict(label='goose grow-only array', file='sum.goose'),
        ],
        cpp=[
            dict(label='cpp vector', file='sum.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp vector+reserve', file='sum.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust Vec, collect', file='sum.rs'),
        ],
    ),
    dict(
        name='push',
        what='Push N records while keeping pointers to every 64th one.',
        sizes=[1000000, 16000000, 64000000],
        zinkite=[dict(label='zinkite array, typed references', file='push.zn')],
        goose=[
            dict(label='goose array, typed references', file='push.goose'),
        ],
        cpp=[
            dict(label='cpp vector, indices', file='push.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp vector+reserve, indices', file='push.cpp', variant=1, tier='expert'),
            dict(label='cpp deque, pointers', file='push.cpp', variant=2, tier='idiomatic'),
        ],
        rust=[
            dict(label='rust Vec+reserve, indices', file='push.rs'),
        ],
    ),
    dict(
        name='strlist',
        what='Split a text into N words, keep them as a list, scan it four times.',
        sizes=[200000, 2000000, 8000000],
        zinkite=[dict(label='zinkite inline strings, owned', file='strlist.zn')],
        goose=[
            dict(label='goose inline strings, owned', file='strlist.goose'),
        ],
        cpp=[
            dict(label='cpp vector<string>', file='strlist.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp vector<string_view>', file='strlist.cpp', variant=1, tier='expert'),
            dict(label='cpp flat offsets', file='strlist.cpp', variant=2, tier='expert'),
        ],
        rust=[
            dict(label='rust Vec<String>', file='strlist_owned.rs'),
            dict(label='rust Vec<&str>', file='strlist_borrowed.rs'),
        ],
    ),
    dict(
        name='records',
        what='Build a log of N variant records, aggregate it four times.',
        sizes=[500000, 4000000, 16000000],
        zinkite=[dict(label='zinkite variable-size class + switch', file='records.zn')],
        goose=[
            dict(label='goose variable enum', file='records_var.goose'),
        ],
        cpp=[
            dict(label='cpp virtual + unique_ptr', file='records.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp variant + string', file='records.cpp', variant=1, tier='idiomatic'),
            dict(label='cpp variant + buffer', file='records.cpp', variant=2, tier='expert'),
        ],
        rust=[
            dict(label='rust enum + String', file='records.rs'),
        ],
    ),
    dict(
        name='tree',
        what='Build a complete binary tree of the given depth, sum it eight times.',
        param='depth',
        sizes=[16, 20, 24],
        zinkite=[dict(label='zinkite links into a static array', file='tree.zn'), dict(label='zinkite i32 indices', file='tree_index.zn')],
        goose=[
            dict(label='goose pool + typed relative refs', file='tree.goose'),
        ],
        cpp=[
            dict(label='cpp unique_ptr', file='tree.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp new/delete', file='tree.cpp', variant=1, tier='idiomatic'),
            dict(label='cpp arena + indices', file='tree.cpp', variant=2, tier='expert'),
        ],
        rust=[
            dict(label='rust Box nodes', file='tree_box.rs'),
            dict(label='rust arena + indices', file='tree_arena.rs'),
        ],
    ),
    dict(
        name='interp',
        what='Build an expression tree of the given depth, evaluate it eight times.',
        param='depth',
        sizes=[16, 20, 24],
        zinkite=[dict(label='zinkite interface calls + references', file='interp.zn'), dict(label='zinkite enum tag + switch + indices', file='interp_tagged.zn')],
        goose=[
            dict(label='goose case functions + relative refs', file='interp.goose'),
        ],
        cpp=[
            dict(label='cpp virtual + unique_ptr', file='interp.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp variant + arena', file='interp.cpp', variant=1, tier='expert'),
            dict(label='cpp tagged union + arena', file='interp.cpp', variant=2, tier='expert'),
        ],
        rust=[
            dict(label='rust enum + Box', file='interp_box.rs'),
            dict(label='rust enum + arena indices', file='interp_arena.rs'),
        ],
    ),
    dict(
        name='graph',
        what='Build adjacency for V vertices and 8V edges, then BFS from four sources.',
        param='V',
        sizes=[100000, 500000, 2000000],
        zinkite=[dict(label='zinkite one-pass, i32 next links', file='graph.zn'), dict(label='zinkite CSR two-pass', file='graph_csr.zn')],
        goose=[
            dict(label='goose one-pass, typed relative refs', file='graph.goose'),
            dict(label='goose CSR two-pass', file='graph_csr.goose'),
        ],
        cpp=[
            dict(label='cpp vector<vector>', file='graph.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp CSR two-pass', file='graph.cpp', variant=1, tier='expert'),
            dict(label='cpp arena + indices', file='graph.cpp', variant=2, tier='expert'),
        ],
        rust=[
            dict(label='rust Vec<Vec>', file='graph_nested.rs'),
            dict(label='rust CSR two-pass', file='graph_csr.rs'),
            dict(label='rust arena + indices', file='graph_arena.rs'),
        ],
    ),
    dict(
        name='words',
        what='Count word frequencies over a text of N words.',
        sizes=[500000, 4000000, 16000000],
        zinkite=[dict(label='zinkite std hashmap', file='words.zn'), dict(label='zinkite slices + open addressing', file='words_open.zn')],
        goose=[
            dict(label='goose slices + open addressing', file='words.goose'),
            dict(label='goose std dictionary', file='words_dictionary.goose'),
        ],
        cpp=[
            dict(label='cpp unordered_map<string>', file='words.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp unordered_map<string_view>', file='words.cpp', variant=1, tier='idiomatic'),
            dict(label='cpp open addressing', file='words.cpp', variant=2, tier='expert'),
        ],
        rust=[
            dict(label='rust HashMap<&str>', file='words_hashmap.rs'),
            dict(label='rust open addressing', file='words_open.rs'),
        ],
    ),
    dict(
        name='particles',
        what='Integrate N particles for 200 steps of f32 vector math.',
        sizes=[100000, 1000000, 4000000],
        zinkite=[dict(label='zinkite AoS, V3 methods', file='particles.zn'), dict(label='zinkite AoS, per component', file='particles_scalar.zn')],
        goose=[
            dict(label='goose AoS, elementwise', file='particles.goose'),
            dict(label='goose AoS, per component', file='particles_scalar.goose'),
        ],
        cpp=[
            dict(label='cpp AoS, per component', file='particles.cpp', variant=0, tier='expert'),
            dict(label='cpp AoS, elementwise', file='particles.cpp', variant=2, tier='idiomatic'),
            dict(label='cpp SoA', file='particles.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust AoS, elementwise', file='particles.rs'),
            dict(label='rust AoS, per component', file='particles_scalar.rs'),
        ],
    ),
    dict(
        name='sexp',
        what='Generate N s-expression forms, parse them into a tree, walk it four times.',
        sizes=[20000, 100000, 300000],
        zinkite=[dict(label='zinkite inline text + links', file='sexp.zn')],
        goose=[
            dict(label='goose pool + refs, inline text', file='sexp.goose'),
        ],
        cpp=[
            dict(label='cpp unique_ptr + string', file='sexp.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp arena + string_view', file='sexp.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust enum + Box + String', file='sexp_box.rs'),
            dict(label='rust arena + &str', file='sexp_arena.rs'),
        ],
    ),
    dict(
        name='lru',
        what='An LRU cache of capacity N/8 under N skewed lookups, with an invalidation every 16th operation.',
        sizes=[2000000, 8000000, 32000000],
        zinkite=[dict(label='zinkite pool + hashmap', file='lru.zn'), dict(label='zinkite array arena + open addressing', file='lru_open.zn')],
        goose=[
            dict(label='goose reusable pool + pool-relative refs', file='lru.goose'),
        ],
        cpp=[
            dict(label='cpp list + unordered_map', file='lru.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp arena + open addressing', file='lru.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust HashMap + arena', file='lru_hashmap.rs'),
            dict(label='rust open addressing + arena', file='lru_open.rs'),
        ],
    ),
    dict(
        name='scene',
        what='Build a scene graph of the given depth, then animate it and recompute world transforms for 16 frames.',
        param='depth',
        sizes=[12, 14, 16],
        zinkite=[dict(label='zinkite links in capped child arrays', file='scene.zn')],
        goose=[
            dict(label='goose pool + inline child refs', file='scene.goose'),
        ],
        cpp=[
            dict(label='cpp unique_ptr children', file='scene.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp arena + child indices', file='scene.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust Box children', file='scene_box.rs'),
            dict(label='rust arena + child indices', file='scene_arena.rs'),
        ],
    ),
    dict(
        name='calc',
        what='Generate, parse and evaluate N small arithmetic expressions, a third of them malformed.',
        sizes=[100000, 400000, 1600000],
        zinkite=[dict(label='zinkite local array + @@', file='calc.zn'), dict(label='zinkite local array + @ and #', file='calc_status.zn')],
        goose=[
            dict(label='goose local pool + return from', file='calc.goose'),
        ],
        cpp=[
            dict(label='cpp unique_ptr + exceptions', file='calc.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp arena + error codes', file='calc.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust Box + Result', file='calc_box.rs'),
            dict(label='rust arena + Result', file='calc_arena.rs'),
        ],
    ),
    dict(
        name='bintrees',
        what='The Benchmarks Game binary-trees: build, check and discard many trees up to the given depth.',
        param='depth',
        sizes=[15, 17, 19],
        zinkite=[dict(label='zinkite local array per tree', file='bintrees.zn'), dict(label='zinkite local array per tree, links', file='bintrees_link.zn')],
        goose=[
            dict(label='goose local pool per tree', file='bintrees.goose'),
        ],
        cpp=[
            dict(label='cpp new/delete', file='bintrees.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp vector arena', file='bintrees.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust Box', file='bintrees_box.rs'),
            dict(label='rust Vec arena', file='bintrees_arena.rs'),
        ],
    ),
    dict(
        name='respond',
        what='Build and render N JSON responses, each a DTO holding a name and a list of line items.',
        sizes=[100000, 400000, 1600000],
        zinkite=[dict(label='zinkite inline DTO', file='respond.zn'), dict(label='zinkite streaming', file='respond_stream.zn')],
        goose=[
            dict(label='goose inline DTO', file='respond.goose'),
            dict(label='goose streaming', file='respond_stream.goose'),
        ],
        cpp=[
            dict(label='cpp DTO + string', file='respond.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp streaming', file='respond.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust DTO + String', file='respond_dto.rs'),
            dict(label='rust streaming', file='respond_stream.rs'),
        ],
    ),
    dict(
        name='blur',
        what='16 passes of a 3x3 blur over a WxW 8-bit image.',
        param='W',
        sizes=[1024, 2048, 8192],
        zinkite=[dict(label='zinkite flat indexing', file='blur.zn'), dict(label='zinkite row slices', file='blur_rows.zn')],
        goose=[
            dict(label='goose flat indexing', file='blur.goose'),
            dict(label='goose row slices', file='blur_rows.goose'),
        ],
        cpp=[
            dict(label='cpp flat indexing', file='blur.cpp', variant=0, tier='idiomatic'),
            dict(label='cpp row pointers', file='blur.cpp', variant=1, tier='expert'),
        ],
        rust=[
            dict(label='rust flat indexing', file='blur_index.rs'),
            dict(label='rust row slices', file='blur_windows.rs'),
        ],
    ),

    # --- classic benchmarks, written for this suite in all four languages ----------
    dict(name="fib", what="Naive doubly recursive Fibonacci of N: function calls and nothing else.",
         param="N", sizes=[32, 37, 43],
         zinkite=[dict(label="zinkite recursion", file="fib.zn")],
         goose=[dict(label="goose recursion", file="fib.goose")],
         cpp=[dict(label="cpp recursion", file="fib.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust recursion", file="fib.rs")]),
    dict(name="levenshtein", what="Edit distance of every pair of N random words, Wagner-Fischer in two rows made per pair.",
         param="words", sizes=[300, 1500, 4000],
         zinkite=[dict(label="zinkite two rows, swapped views", file="levenshtein.zn")],
         goose=[dict(label="goose two rows, swapped slices", file="levenshtein.goose")],
         cpp=[dict(label="cpp two vectors, swap", file="levenshtein.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust two Vecs, mem::swap", file="levenshtein.rs")]),
    dict(name="nbody", what="Benchmarks Game n-body: N steps of five Jovian bodies in f64.",
         param="steps", sizes=[100000, 5000000, 20000000],
         zinkite=[dict(label="zinkite fixed array of classes", file="nbody.zn")],
         goose=[dict(label="goose fixed array of structs", file="nbody.goose")],
         cpp=[dict(label="cpp std::array of structs", file="nbody.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust array of structs", file="nbody.rs")]),
    dict(name="mandelbrot", what="Benchmarks Game Mandelbrot: an N x N bitmap, 50 iterations, checksummed with FNV-1a.",
         param="N", sizes=[500, 2000, 5000],
         zinkite=[dict(label="zinkite per-pixel loop", file="mandelbrot.zn")],
         goose=[dict(label="goose per-pixel loop", file="mandelbrot.goose")],
         cpp=[dict(label="cpp per-pixel loop", file="mandelbrot.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust per-pixel loop", file="mandelbrot.rs")]),
    dict(name="spectralnorm", what="Benchmarks Game spectral norm: ten power-method rounds on an N x N matrix made on the fly.",
         param="N", sizes=[500, 2000, 5500],
         zinkite=[dict(label="zinkite row by row", file="spectralnorm.zn")],
         goose=[dict(label="goose row by row", file="spectralnorm.goose")],
         cpp=[dict(label="cpp row by row", file="spectralnorm.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust row by row", file="spectralnorm.rs")]),
    dict(name="nqueen", what="Count the solutions of the n-queens problem by iterative backtracking over bit masks.",
         param="n", sizes=[12, 14, 15],
         zinkite=[dict(label="zinkite bit masks", file="nqueen.zn")],
         goose=[dict(label="goose bit masks", file="nqueen.goose")],
         cpp=[dict(label="cpp bit masks", file="nqueen.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust bit masks", file="nqueen.rs")]),
    dict(name="fannkuch", what="fannkuch-redux: count pancake flips over every permutation of n elements.",
         param="n", sizes=[9, 10, 11],
         zinkite=[dict(label="zinkite arrays, slice reverse", file="fannkuch.zn")],
         goose=[dict(label="goose fixed arrays, reverse", file="fannkuch.goose")],
         cpp=[dict(label="cpp vector, std::reverse", file="fannkuch.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust Vec, slice reverse", file="fannkuch.rs")]),
    dict(name="nsieve", what="Sieve of Eratosthenes over 10000<<N, <<N-1 and <<N-2 byte flags.",
         param="N", sizes=[6, 10, 13],
         zinkite=[dict(label="zinkite bool array", file="nsieve.zn")],
         goose=[dict(label="goose bool array", file="nsieve.goose")],
         cpp=[dict(label="cpp vector<uint8_t>", file="nsieve.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust Vec<bool>", file="nsieve.rs")]),
    dict(name="matmul", what="Multiply two n x n f64 matrices of small integers by the i-k-j loop.",
         param="n", sizes=[300, 700, 1500],
         zinkite=[dict(label="zinkite row slices", file="matmul.zn")],
         goose=[dict(label="goose simd fn, row slices", file="matmul.goose")],
         cpp=[dict(label="cpp flat vector", file="matmul.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust chunks + zip", file="matmul.rs")]),
    dict(name="base64", what="Encode N random bytes as base64 and decode them back, 16 times each.",
         param="bytes", sizes=[100000, 2000000, 24000000],
         zinkite=[dict(label="zinkite zink.encoding", file="base64.zn")],
         goose=[dict(label="goose std base64 (simd)", file="base64_std.goose")],
         cpp=[dict(label="cpp hand-written tables", file="base64.cpp", variant=0, tier="idiomatic")],
         rust=[dict(label="rust hand-written tables", file="base64.rs")]),
]
