# Federation envelope signing

Status: implemented in crates/federation/src/envelope.rs; pinned by
crates/federation/tests/signing_vectors.rs. This document is the canonical
form ADR-005 refers to.

## What a signature covers

One signature per envelope, over the canonical JSON bytes of the envelope with
its signature field set to the empty string. Everything else is signed,
including protocol, schema_version, sender, recipient, issued_at_ms,
expires_at_ms, correlation, idempotency_key, content_type, and payload.

Signing the protocol and schema binds the version: a receiver that does not
speak the version fails before it trusts any field.

## Canonical bytes

1. Serialize the envelope as JSON in the field order of the Envelope struct
   (declaration order, which serde preserves for structs):

       protocol, schema_version, message_id, kind, sender, recipient,
       issued_at_ms, expires_at_ms, correlation, idempotency_key,
       content_type, payload, signature

2. Set signature to the empty string.
3. Canonicalize the payload recursively before serializing: object keys are
   sorted lexicographically by Unicode code point; array order is preserved.
   A parser that preserved a different map order therefore still signs the
   same bytes.
4. Encode as UTF-8 JSON with no insignificant whitespace: no spaces after
   separators, no trailing newline. Strings carry only the escapes JSON
   requires (quote, backslash, control characters below 0x20); non-ASCII text
   is written as UTF-8, not escaped.
5. Numbers are the decimal form serde_json emits. Integers are exact within
   i64/u64; a protocol field that must be exact is an integer, never a float.

The empty-signature rule means the signed bytes always end with
"signature":""}.

## Signature encoding

Ed25519 over the bytes above, using the sender's 32-byte seed. The signature is
the 64-byte Ed25519 output encoded with standard base64 including padding.
The sender field is the base64 of the sender's public key, so the key is read
from the envelope, not looked up.

## Verification order

A receiver must check, in this order, and refuse on the first failure:

1. The bytes decode as JSON and as an Envelope.
2. protocol is exactly bsh.fed/1.
3. schema_version is exactly 1.
4. The signature verifies over the canonical bytes.
5. expires_at_ms, when present, is strictly after now.
6. issued_at_ms is not in the future beyond the allowed clock skew.

Only after all six may a router act on kind, sender, or payload. A signature
that verifies says who sent the envelope; it never says that the sender may
invoke what the payload asks for.

## Test vector

Seeds: sender = 32 bytes of 0x07, recipient = 32 bytes of 0x09. Message:
kind invoke, id msg-vector-1, correlation corr-1, issued at 1700000000000,
expires at 1700000060000 (the default 60 s lifetime), payload

    {"capability":"builtin/read_file","input":{"path":"a.txt"}}

Canonical bytes (one line, no trailing newline):

    {"protocol":"bsh.fed/1","schema_version":1,"message_id":"msg-vector-1","kind":"invoke","sender":"6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=","recipient":"/RckOFqgx1tk+3jNYC+h2ZH96/drE8WO1wLqyDXp9hg=","issued_at_ms":1700000000000,"expires_at_ms":1700000060000,"correlation":"corr-1","idempotency_key":null,"content_type":"application/json","payload":{"capability":"builtin/read_file","input":{"path":"a.txt"}},"signature":""}

Signature (base64, standard padding):

    FdehIT5DHeNGNx+NbDvQh7l+LGXAjFjcmkUFrzu9fSjQX66Qh83raQl+Gg8dBmvgCPD7Z6Iu6ttF/3EWZnjDCw==

## Interoperability checklist

- Field order matches the list above; a struct serializer that sorts struct
  fields would break the vector.
- Payload object keys are sorted before signing.
- No BOM, no trailing newline, no spaces after ':' or ','.
- Base64 is standard with padding, not URL-safe.
- Verification checks protocol and schema before the signature so a version
  mismatch reads as such, not as a bad signature.
