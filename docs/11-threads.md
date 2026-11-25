# 11. Threads

The `zink.thread` package runs work on threads of their own. A worker shares no memory with
the thread that started it: it gets copies of what it uses when it starts, and values go
between threads through queues, copied in and out. So no reference ever crosses a thread,
and the language needs no locks, no atomics and no rules about data races. This chapter
covers starting workers with `thread.spawn`, what a worker can take with it, the program's
statics in a worker, joining and statuses, `queue<T>`, Files and output, and the usual
patterns, ending with `examples/hashtree.zn`. The reasons are in
[DESIGN.md §12.9](../DESIGN.md#129-threads-decided-2026-11-24), and the library's design in
[STDLIB.md §10.3](../STDLIB.md#103-threads-zinkthread-v1).

## Starting a worker: `thread.spawn`

`thread.spawn` takes a lambda with no parameters and runs it on a new thread. It gives a
`Worker`, whose `join()` waits for it to end:

```java
import zink.thread.*;

i64 sumTo(i64 n) {
    i64 s = 0;
    for (i64 i = 1; i <= n; i++) {
        s += i;
    }
    return s;
}

void main() {
    queue<i64> results = [];
    Worker[..>] workers = [];
    for (i64 k = 1; k <= 4; k++) {
        workers +> thread.spawn(() => results.put(sumTo(k * 1000)));
    }
    i64 total = 0;
    for (i64 k = 1; k <= 4; k++) {
        total += results.get();
    }
    for (Worker& w : workers) {
        w.join();
    }
    println("total ${total}, on a machine with ${thread.cores() > 0 ? "some" : "no"} cores");
}
```

```output
total 15005000, on a machine with some cores
```

- **`import zink.thread.*;`** gives `thread`, `Worker` and the built-in `queue<T>`. A program
  that doesn't import it is compiled exactly as before, with no cost for threads.
- **The lambda is the work.** It has no parameters, and what it uses from outside is copied
  into the new thread when it starts. Method references are undecided, so a function is
  named by calling it inside a lambda, as `() => work(jobs, results)`; passing `work` itself
  is an error:

```java
import zink.thread.*;

void work() {
    println("working");
}

void main() {
    Worker w = thread.spawn(work);
    w.join();
}
```

```error
example.zn:8:16: error: `thread.spawn` takes one lambda with no parameters
help: write `thread.spawn(() => work(a, b))`: what the lambda uses from outside is copied to the new thread (STDLIB §10.3)
```

- **A loop's counter is copied at each spawn.** Each worker above has the `k` it was spawned
  with, though the loop goes on changing it. Inside the lambda, what it captured is
  read-only, as in every lambda; the spawning thread may change its own variables
  afterwards, and the worker's copies don't change with them.
- **`thread.cores()`** is how many processors the system has online, for sizing a pool.
- **A worker's own variables** are on data stacks of its own, which are given back to the
  system when it ends.

## What a worker takes with it

What crosses into a worker must be **flat**: a value with nothing in it that refers into the
spawning thread's memory. These are flat:

- numbers, `bool` and enums;
- queues and `Worker`s;
- capped arrays (`u8[..~16]`) and fixed-size arrays (`i64[.3]`) of flat values, which are
  stored inline;
- classes made only of those.

```java
import zink.thread.*;

enum Shape { SQUARE, CIRCLE }

class Spec {
    Shape shape;
    f64 size;
    bool rounded;
    u8[..~16] label;
}

f64 area(Spec s) {
    return s.shape == Shape.SQUARE ? s.size * s.size : 3.0 * s.size * s.size;
}

void main() {
    Spec spec = new Spec(Shape.CIRCLE, 2.0, true, []);
    spec.label.append("wheel");
    i64[.3] weights = [1, 2, 3];
    queue<f64> out = [];
    Worker w = thread.spawn(() => {
        out.put(area(spec) * (f64) weights[2]);
        println("${spec.label}: ${spec.rounded}");
    });
    f64 a = out.get();
    w.join();
    println("${a}");
}
```

```output
wheel: true
36.0
```

Anything else is a compile error that names what refers into the thread's memory:

```java
import zink.thread.*;

void main() {
    string name = "pool";
    Worker w = thread.spawn(() => println(name));
    w.join();
}
```

```error
example.zn:5:43: error: a worker can't take `name` with it: `string` is a view into this thread's memory
help: what a spawned lambda uses from outside is copied to the new thread, so it must be flat: numbers, bool, enums, queues, capped arrays, and classes made only of those (STDLIB §10.3)
```

The other messages, for what can't cross:

| Captured | Message |
|---|---|
| a `string` or other view | `` a worker can't take `name` with it: `string` is a view into this thread's memory `` |
| a growable or frozen array, `i64[..>]` or `i64[.]` | `` a worker can't take `xs` with it: `i64[..>]` is on one of this thread's data stacks `` |
| a `hashmap`, `hashset` or `pool` | `` `hashmap<string, i64>` is on one of this thread's data stacks `` |
| a reference, `i64&` | `` `i64&` refers into this thread's memory `` |
| a class with a `string` field | `` `Named`'s field `name`: `string` is a view into this thread's memory `` |
| a `File` | `` a `File` belongs to the thread that opened it `` |
| a `func` value | `` `func(i64 => i64)` is a function value `` |
| a C handle ([Calling C](12-calling-c.md)) | `` `FILE` is a C pointer `` |
| a field, in a method | `` a worker can't take `this` with it: it refers into the spawning thread's memory `` |

- **Text travels in a capped array**, as `u8[..~4096]` does in `examples/hashtree.zn` (see
  [Text in messages](#text-in-messages)).
- **A method's fields are reached through `this`**, which refers into the spawning thread's
  memory. Copy the fields the worker needs into flat locals before `thread.spawn`; the help
  says so.

## Statics in a worker

A new thread gets a copy of each of the program's statics whose type allows it, taken when
it is spawned:

- flat values;
- views of string literals;
- arrays on data stacks of those, elements included, which go on data stacks of the new
  thread's own.

What a worker changes stays its own, and what the spawning thread changes afterwards
doesn't reach it:

```java
import zink.thread.*;

class Kind {
    string name;
    i64 weight;
}

i64 counter = 10;
string GREETING = "hello";
Kind[..>] KINDS = [];
i64[..>] PRIMES = [];

void main() {
    KINDS +> new Kind("stone", 3) +> new Kind("dirt", 1);
    PRIMES +> 2 +> 3 +> 5;
    counter = 42;
    queue<i64> back = [];
    Worker w = thread.spawn(() => {
        counter += 1;
        PRIMES +> 7;
        back.put(counter);
        println("worker: ${GREETING}, ${KINDS[0].name}, ${PRIMES}, counter ${counter}");
    });
    i64 theirs = back.get();
    w.join();
    PRIMES +> 11;
    println("main: ${PRIMES}, counter ${counter}, the worker's ${theirs}");
}
```

```output
worker: hello, stone, [2, 3, 5, 7], counter 43
main: [2, 3, 5, 11], counter 42, the worker's 43
```

A static the new thread doesn't get, such as a map, would start empty there, so a worker
that reaches one, directly or through anything it calls, is a compile error. The checker
follows the calls the worker's lambda makes, through interfaces too:

```java
import zink.thread.*;

hashmap<string, i64> cache = [];

i64 lookup(string key) {
    i64 v @_ = cache.get(key);
    return v;
}

void main() {
    cache.put("a", 1);
    Worker w = thread.spawn(() => println("${lookup("a")}"));
    w.join();
}
```

```error
example.zn:6:16: error: the worker spawned on line 12 can't use the static `cache`: a new thread doesn't get a copy of a `hashmap<string, i64>`, so it would start empty there
help: pass the worker what it needs when it's spawned or through a queue, or keep it in a static of numbers, classes of them, or an array of those (STDLIB §10.3)
```

A static that holds views of anything but string literals can't be used either, since its
views would point into the spawning thread's memory:

```java
import zink.thread.*;

u8[..>] buffer = [];
string first = "";

void main() {
    buffer.append("hello");
    first = buffer[0:2];
    Worker w = thread.spawn(() => println(first));
    w.join();
}
```

```error
example.zn:9:43: error: the worker spawned on line 9 can't use the static `first`: it holds views that may point into the spawning thread's memory
help: only views of string literals are shared between threads; copy the text into a capped array (STDLIB §10.3)
```

- **The copy is taken at the spawn**, so the static's value in the worker is what it was at
  that moment, `counter` 42 above, not its initializer's.
- **Statics the worker never uses are copied all the same.** A large static array is copied
  into every worker; pass what a worker needs through its lambda or a queue instead.
- **Calls through `func` values are followed conservatively**: a worker that calls through
  one is checked as if it could call any lambda in the program.

## Joining and statuses

Failures follow [chapter 7](07-errors.md) in each thread. `w.join()` waits for the worker to
end, and fails with the status its lambda ended with:

```java
import zink.thread.*;

i64 parseAll(u8[..~64] text) {
    i64 sum = 0;
    for (string word : text.words()) {
        i64 n @err = i64.parse(word);
        if (err) {
            return #err sum;
        }
        sum += n;
    }
    return sum;
}

void check(i64 n) {
    if (n < 0) {
        return #status.RANGE;
    }
}

void main() {
    u8[..~64] good = [];
    good.append("1 2 3");
    u8[..~64] bad = [];
    bad.append("4 five 6");
    queue<i64> sums = [];
    Worker a = thread.spawn(() => {
        i64 s @err = parseAll(good);
        sums.put(s);
        return #err;
    });
    Worker b = thread.spawn(() => {
        i64 s @err = parseAll(bad);
        sums.put(s);
        return #err;
    });
    Worker c = thread.spawn(() => { return ## check(-1); });
    i64 total = sums.get() + sums.get();
    u8 ea = @a.join();
    u8 eb = @b.join();
    u8 ec = @c.join();
    println("${total} ${status.name(ea)} ${status.name(eb)} ${status.name(ec)}");
}
```

```output
10 OK INVALID RANGE
```

- **`return #err;` and `return ## f();`** in the lambda give the worker's status, as in any
  lambda that returns nothing.
- **`join()` can be called any number of times, from any thread.** A `Worker` is flat, so it
  can be put in a queue or passed to another worker.
- **A `join()` nothing reads** stops the program if the worker failed, at the line of the
  `join`: here that would be "`` `join` failed with INVALID, and nothing read its status ``".
- **A zero-value `Worker`**, such as a failed `poll` of a `queue<Worker>` gives, fails `join`
  with `INVALID`.
- **`thread.spawn` itself fails** when the system can't start a thread: its status is the
  system's error, `AGAIN` as a rule, and the `Worker` it gives is the zero value.

### A failure nothing reads in a worker

A failure nothing reads stops the whole program, with its message and trail, as it would in
`main`:

```java
import zink.thread.*;

i64 parse(u8[..~16] text) {
    return ## i64.parse(text);
}

void main() {
    u8[..~16] text = [];
    text.append("seven");
    Worker w = thread.spawn(() => {
        i64 n = parse(text);
        println("got ${n}");
    });
    w.join();
    println("done");
}
```

```stderr exit=1
example.zn:11: `parse` failed with INVALID, and nothing read its status
    `parse` returned it at example.zn:4
```

### `@@` in a worker

A worker that should survive failures in its work puts `@@` around each piece of it, so a
bad job fails that job only:

```java
import zink.thread.*;

class Job {
    i64 id;
    i64 divisor;
}

class Answer {
    i64 id;
    i64 value;
    u8 err;
}

i64 compute(Job j) {
    i64[.] table = [100, 200, 300];
    return table[j.id] / j.divisor;
}

void main() {
    queue<Job> jobs = [];
    queue<Answer> answers = [];
    Worker w = thread.spawn(() => {
        while (Job j : jobs.get()) {
            i64 v @@err = compute(j);
            answers.put(new Answer(j.id, v, err));
        }
    });
    jobs.put(new Job(0, 4));
    jobs.put(new Job(1, 0));
    jobs.put(new Job(7, 1));
    jobs.put(new Job(2, 3));
    jobs.close();
    for (i64 i = 0; i < 4; i++) {
        Answer a = answers.get();
        println("job ${a.id}: ${a.value} ${status.name(a.err)}");
    }
    w.join();
}
```

```output
job 0: 25 OK
job 1: 0 DOMAIN
job 7: 0 RANGE
job 2: 100 OK
```

The status travels back in the answer, a `u8` field, since a status is a number like any
other.

## Queues: `queue<T>`

A `queue<T>` carries values between threads, first in, first out. It is built in, made with
`[]`, and holds a flat `T`:

```java
queue<Job> jobs = [];
```

| Call | Does | Fails with |
|---|---|---|
| `q.put(v)` | copies `v` in; never waits | `FULL` at the limit, `END` once closed |
| `q.get()` | waits for the oldest value | `END` once closed and empty |
| `q.get(timeout)` | waits at most a `Duration` | `TIMEOUT`, or `END` |
| `q.poll()` | takes the oldest value if there is one now | `EMPTY`, or `END` once closed and empty |
| `q.close()` | no more puts; gets end once it's empty | |
| `q.limit(n)` | at most `n` queued, 0 for no limit | `INVALID` below 0 |
| `q.len` | how many are queued now | |

```java
import zink.thread.*;
import zink.time.*;

void main() {
    queue<i64> q = [];
    i64 a @e1 = q.poll();
    println("poll on an empty queue: ${a} ${status.name(e1)}");
    i64 b @e2 = q.get(Duration.ms(10));
    println("get with a timeout: ${b} ${status.name(e2)}");
    q.limit(2);
    q.put(1);
    q.put(2);
    u8 e3 = @q.put(3);
    println("put at the limit: ${status.name(e3)}, ${q.len} queued");
    u8 e4 = @q.limit(-1);
    println("limit(-1): ${status.name(e4)}");
    q.close();
    u8 e5 = @q.put(4);
    println("put once closed: ${status.name(e5)}");
    while (i64 v : q.get()) @why {
        println("got ${v}");
    }
    println("then ${status.name(why)}");
    i64 c @e6 = q.poll();
    println("poll once closed and empty: ${c} ${status.name(e6)}");
}
```

```output
poll on an empty queue: 0 EMPTY
get with a timeout: 0 TIMEOUT
put at the limit: FULL, 2 queued
limit(-1): INVALID
put once closed: END
got 1
got 2
then END
poll once closed and empty: 0 END
```

- **`queue` needs the import.** Without it, `queue<i64> q = [];` is "`queue` is a type of
  `zink.thread`", with the help "add `import zink.thread.*;`".
- **The values must be flat**, for the same reason a worker's captures must be:

```java
import zink.thread.*;

class Named {
    string name;
    i64 n;
}

void main() {
    queue<Named> q = [];
}
```

```error
example.zn:9:5: error: a queue's values must be flat, and `Named`'s field `name`: `string` is a view into this thread's memory
help: a queue copies its values from one thread to another: numbers, bool, enums, queues, capped arrays, and classes made only of those (STDLIB §10.3)
```

- **A queue is a handle.** Copies share the queue, so a worker takes one along like any flat
  value, and a queue can travel inside a class, or through another queue.
- **A queue lasts for the whole run**, and its memory isn't given back. Make queues once,
  not one per message.
- **`get` on a failure gives the zero value**, as the library always does.
- **A queue that is a zero value**, such as a field of a class that a failed `poll` gave,
  fails `put`, `get`, `poll` and `limit` with `INVALID`; its `len` is 0, and `close()` does
  nothing.
- **`get(timeout)` needs `import zink.time.*;`** for `Duration` ([The library: files,
  processes, time and the command line](13-library-system.md)).

### Closing a queue ends the workers' loops

`while (Job j : jobs.get())` stops at `END` once the queue is closed and every value in it
has been taken, so no quit message is needed. Close the queue after the last `put`, then
join the workers. Each pattern below works this way.

## Files and output

- **A `File` belongs to the thread that opened it.** It can't be captured or put in a queue,
  and the thread's open Files are written out and closed when it ends:

```java
import zink.io.*;
import zink.thread.*;

void main() {
    u8[..~64] path = [];
    path.append("/tmp/zink-docs-11-worker.txt");
    Worker w = thread.spawn(() => {
        File f = File.create(path);
        f.println("written by a worker");
    });
    w.join();
    u8[.] text = File.readAll(path);
    print(text);
    fs.remove(path);
}
```

```output
written by a worker
```

- **`stdin`, `stdout` and `stderr` work in every thread.**
- **Output lines stay whole.** One thread's `println` is written as one line, never mixed
  with another's, though the order of lines from different workers varies from run to run.
  This program prints twelve whole lines, in some order:

```java
import zink.thread.*;

void main() {
    Worker[..>] ws = [];
    for (i64 t = 0; t < 4; t++) {
        ws +> thread.spawn(() => {
            for (i64 k = 0; k < 3; k++) {
                println("worker ${t}, line ${k}: each line comes out whole");
            }
        });
    }
    for (Worker& w : ws) {
        w.join();
    }
}
```

- **Workers still running when `main` returns are stopped with the program**, and what
  their Files hold unwritten is lost. Join them first:

```java
import zink.thread.*;
import zink.time.*;

void main() {
    Worker w = thread.spawn(() => {
        time.sleep(Duration.seconds(5));
        println("never printed");
    });
    println("main returns");
}
```

```output
main returns
```

## Patterns

The examples below make their output the same on every run, whatever order the workers
finish in: they sum results, sort them, or have one worker do the printing.

### A pool of workers over a job queue

A fixed number of workers take jobs from one queue and put results in another. The main
thread puts every job, closes the queue, and takes exactly as many results:

```java
import zink.thread.*;

class Job {
    i64 n;
}

class Done {
    i64 n;
    i64 steps;
}

// How many steps the Collatz sequence takes from n down to 1.
i64 collatz(i64 n) {
    i64 steps = 0;
    i64 x = n;
    while (x != 1) {
        x = x % 2 == 0 ? x / 2 : 3 * x + 1;
        steps++;
    }
    return steps;
}

void main() {
    queue<Job> jobs = [];
    queue<Done> done = [];
    Worker[..>] pool = [];
    for (i64 i = 0; i < 4; i++) {
        pool +> thread.spawn(() => {
            while (Job j : jobs.get()) {
                done.put(new Done(j.n, collatz(j.n)));
            }
        });
    }
    for (i64 n = 1; n <= 10000; n++) {
        jobs.put(new Job(n));
    }
    jobs.close();
    Done best = new Done(0, 0);
    for (i64 i = 0; i < 10000; i++) {
        Done d = done.get();
        if (d.steps > best.steps || (d.steps == best.steps && d.n < best.n)) {
            best = d;
        }
    }
    for (Worker& w : pool) {
        w.join();
    }
    println("below 10001, ${best.n} takes the most steps: ${best.steps}");
}
```

```output
below 10001, 6171 takes the most steps: 261
```

`thread.cores()` is the usual size of a pool for work that keeps a processor busy.

### Fan-out and fan-in

Each worker takes a part of the work given by its loop counter, and puts one result. The
main thread collects them and sorts them by part:

```java
import zink.thread.*;

class Part {
    i64 index;
    i64 primes;
}

bool isPrime(i64 n) {
    if (n < 2) {
        return false;
    }
    for (i64 d = 2; d * d <= n; d++) {
        if (n % d == 0) {
            return false;
        }
    }
    return true;
}

void main() {
    i64 parts = 8;
    i64 size = 25000;
    queue<Part> results = [];
    Worker[..>] pool = [];
    for (i64 p = 0; p < parts; p++) {
        pool +> thread.spawn(() => {
            i64 count = 0;
            for (i64 n = p * size; n < (p + 1) * size; n++) {
                if (isPrime(n)) {
                    count++;
                }
            }
            results.put(new Part(p, count));
        });
    }
    Part[..>] got = [];
    for (i64 p = 0; p < parts; p++) {
        got +> results.get();
    }
    for (Worker& w : pool) {
        w.join();
    }
    got.sortBy((Part x) => x.index);
    i64 total = 0;
    for (Part& r : got) {
        println("${r.index * size} to ${(r.index + 1) * size - 1}: ${r.primes}");
        total += r.primes;
    }
    println("below ${parts * size}: ${total}");
}
```

```output
0 to 24999: 2762
25000 to 49999: 2371
50000 to 74999: 2260
75000 to 99999: 2199
100000 to 124999: 2142
125000 to 149999: 2114
150000 to 174999: 2068
175000 to 199999: 2068
below 200000: 17984
```

### Workers that spawn workers

A worker can spawn workers of its own, and pass on the queues it was given. Here one worker
starts a team, waits for it, and closes the queue, which ends the main thread's loop:

```java
import zink.thread.*;

i64 sumOfSquares(i64 from, i64 to) {
    i64 s = 0;
    for (i64 i = from; i < to; i++) {
        s += i * i;
    }
    return s;
}

void main() {
    queue<i64> sums = [];
    Worker boss = thread.spawn(() => {
        Worker[..>] team = [];
        for (i64 t = 0; t < 4; t++) {
            team +> thread.spawn(() => sums.put(sumOfSquares(t * 250, (t + 1) * 250)));
        }
        for (Worker& w : team) {
            w.join();
        }
        sums.close();
    });
    i64 total = 0;
    i64 parts = 0;
    while (i64 s : sums.get()) {
        total += s;
        parts++;
    }
    boss.join();
    println("${parts} parts, sum of squares below 1000: ${total}");
}
```

```output
4 parts, sum of squares below 1000: 332833500
```

### Bounding a producer with `limit`

A producer that is faster than its workers would fill a queue without end. `limit(n)` makes
`put` fail with `FULL` instead of waiting, and while it does, the producer takes a result,
so it never waits on a full queue while the workers wait on it:

```java
import zink.thread.*;

class Job {
    i64 n;
}

void main() {
    queue<Job> jobs = [];
    queue<i64> results = [];
    jobs.limit(8);
    Worker[..>] pool = [];
    for (i64 i = 0; i < 3; i++) {
        pool +> thread.spawn(() => {
            while (Job j : jobs.get()) {
                results.put(j.n * 2);
            }
        });
    }
    i64 queued = 0;
    i64 total = 0;
    for (i64 n = 1; n <= 1000; n++) {
        while (@jobs.put(new Job(n)) == status.FULL) {
            total += results.get();
            queued--;
        }
        queued++;
    }
    jobs.close();
    while (queued > 0) {
        total += results.get();
        queued--;
    }
    for (Worker& w : pool) {
        w.join();
    }
    println("total ${total}, with at most 8 jobs waiting");
}
```

```output
total 1001000, with at most 8 jobs waiting
```

`queued` counts the jobs whose results haven't been taken, so a `get` while `jobs` is full
always has a result coming.

### Text in messages

A `string` is a view, so text goes into a message as a capped `u8` array. `append` copies all
of the text or none of it, and fails with `FULL` when it doesn't fit:

```java
import zink.thread.*;

class Job {
    u8[..~32] word;
}

class Count {
    u8[..~32] word;
    i64 vowels;
}

i64 vowels(string w) {
    i64 n = 0;
    for (u8 c : w) {
        if (c == 'a' || c == 'e' || c == 'i' || c == 'o' || c == 'u') {
            n++;
        }
    }
    return n;
}

void main() {
    queue<Job> jobs = [];
    queue<Count> counts = [];
    Worker w = thread.spawn(() => {
        while (Job j : jobs.get()) {
            counts.put(new Count(j.word, vowels(j.word)));
        }
    });
    string text = "a queue copies each value from one thread to another";
    i64 sent = 0;
    for (string word : text.words()) {
        Job j = new Job([]);
        j.word.append(word);
        jobs.put(j);
        sent++;
    }
    Job tooLong = new Job([]);
    u8 err = @tooLong.word.append("supercalifragilisticexpialidocious");
    println("a long word: ${status.name(err)}, ${tooLong.word.len} bytes kept");
    jobs.close();
    for (i64 i = 0; i < sent; i++) {
        Count c = counts.get();
        print("${c.word}:${c.vowels} ");
    }
    println("");
    w.join();
}
```

```output
a long word: FULL, 0 bytes kept
a:1 queue:4 copies:3 each:2 value:3 from:1 one:2 thread:2 to:1 another:3 
```

- **A capped array costs its whole capacity** in every message, whatever its length: a
  `u8[..~4096]` path is 4 KB.
- **A capped `u8` array works where a `string` is wanted**, as `vowels(j.word)` and
  `print("${c.word}")` show.

## A complete tool: `examples/hashtree.zn`

`hashtree` prints the SHA-256 of every file under a directory, as `sha256sum` does, sorted
by path, with a pool of workers doing the hashing. Its messages carry paths in capped
arrays:

```java
class Job {
    u8[..~4096] path;
}

class Result {
    u8[..~4096] path;
    u8[.32] digest;
    u8 err;                 // 0, or why the file couldn't be read
}
```

Each worker takes paths until the queue is closed and empty, opens each file itself, and
hashes it 64 KB at a time with one buffer of its own:

```java
void work(queue<Job> jobs, queue<Result> results) {
    u8[..>] piece = [];
    while (Job j : jobs.get()) {
        ...
        File f @openErr = File.open(j.path);
        if (openErr != 0) {
            r.err = openErr;
            results.put(r);
            continue;
        }
        ...
    }
}
```

The main thread starts the pool, walks the tree, and puts each file's path in the queue.
The limit keeps the walk a little ahead of the hashing, and while a `put` fails with `FULL`
it collects a result:

```java
queue<Job> jobs = [];
queue<Result> results = [];
jobs.limit(4 * workers);
Worker[..>] pool = [];
for (i64 i = 0; i < workers; i++) {
    pool +> thread.spawn(() => work(jobs, results));
}
...
while (@jobs.put(j) == status.FULL) {
    failed = collect(results, names, rows) || failed;
    queued--;
}
```

Then it closes the queue, collects the rest, joins the workers, sorts the rows by path and
prints them:

```
$ zink run -O examples/hashtree.zn -j 4 compiler/lib/zink/thread
09d865a8cf70c1739184c080e0eacfedb599f81d8aba2a316f308e55eba614a6  compiler/lib/zink/thread/Worker.zn
5e2ac1745f26b79359fae164de6f19a63e0a9bbc0cfd59988cd1eb7f92e9fc7c  compiler/lib/zink/thread/thread.zn
```

- **It matches `sha256sum`** on the 12,103 files of `/usr/include`, 152 MB, and hashes them
  in 0.69 s with one worker and 0.10 s with 18.
- **A file that can't be read** is reported on stderr with `status.message`, and makes the
  exit status 1. Its status travels back in `Result.err`.
- **ThreadSanitizer finds nothing in it**, built with it.
- [examples/NOTES.md §8](../examples/NOTES.md#8-hashtree-threads) says what writing it
  showed.

## How it works

A program that imports `zink.thread` is compiled with the runtime's per-thread state, its
data stacks, scratch stack, failure trail, `@@` records and open Files, and the program's
statics, all thread-local, so each worker is the program over again. A spawned lambda
becomes a C function given a packet that the spawning thread filled with its captures' bytes
and the statics', which it unpacks into its own. Workers are detached POSIX threads with a
record each that `join` waits on. A queue is a mutex, a condition variable and a list of
nodes, each one value's bytes. Output takes stdout's lock a line at a time, and the
environment has a lock of its own. A worker's data stacks are unmapped when it ends: 300
workers in turn that each fill 8 MB peak at 10 MB.

## Not yet

- **Variable-size values through queues**, which would carry only a text's bytes rather
  than a capped array's whole capacity.
- **Workers that read the parent's arrays in place**, for kernels that split one large
  array, where copying it into messages and back costs more than the threads gain. This is
  open ([DESIGN §17](../DESIGN.md#17-open-decisions)).
- **Method references** as the work, `thread.spawn(this::run)`.

[What isn't there yet](15-not-yet.md) collects these with the rest.
