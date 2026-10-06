---
module: jsbos
tags: [bindings, nodejs, concurrency, tokio, lock, testing]
problem_type: bug
---

# Never take a blocking lock in a native getter

## Problem

Reading `sub.topic` from JavaScript while a receive was in flight silently
broke delivery. The subscriber was created and awaited correctly, but received
nothing:

```js
const sub = await bus.createSubscriber('demo/events')
const pending = sub.recvWithTimeoutMs(5000)
await sleep(200)
await bus.publishText(sub.topic, 'hello') // never arrived
const msg = await pending // null
```

Publishing the same string from a literal worked. Publishing through a
`Publisher` object worked. Only the getter value failed, which made it look
like a topic-string problem rather than a locking one.

## Root cause

`crates/jsbos/src/subscriber.rs` stores the receiver behind
`Arc<tokio::sync::Mutex<bus::Subscriber<String>>>`. The `topic()` getter called
`self.inner.blocking_lock()`, while `recv_with_timeout_ms()` holds that same
lock across its await. Reading the topic therefore blocked the JavaScript
thread until the in-flight receive timed out. By the time the getter returned,
the receive had already resolved to null, so the publish that followed landed
after the subscribe window had closed.

The failure is confusing because the getter returns the correct value, just too
late, and any concurrent `recv` or `recvJson` on the same object loses its
pending message.

## Fix

The topic never changes, so the wrapper caches it in a plain field and the
getter clones that cache instead of touching the lock. `crates/nbos` already
did this: its `PySubscriber` caches `topic` with the comment that reading it
must not wait for a pending recv.

Only wrappers whose inner value sits behind a mutex need the cache. `Publisher`
and `Query` hold their inner value directly, so their getters never contend.
The general rule: a native getter must not call `blocking_lock` on state that a
concurrent async method can hold across an await. Cache immutable state at
construction time instead.

## Guard

`crates/jsbos/test/bus.test.js` publishes to `sub.topic` while a receive is
pending, so a regression fails the JS suite instead of silently dropping
messages.