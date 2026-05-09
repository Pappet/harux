## 2026-05-08 - [O(1) Dictionary Lookup]
**Learning:** O(1) dictionary lookup for HL7 field descriptions using direct indexing can be risky because fields might not always be perfectly ordered without gaps. The correct approach is to attempt an O(1) direct index lookup, but gracefully fallback to an O(n) iter().find() if the field sequence does not match the index.
**Action:** Always consider the fallback mechanisms when introducing structural assumptions for optimization, especially around array bounds or specific sequences.

## 2026-05-08 - [Lift Expensive Lookups Out of Tight Loops]
**Learning:** Calling a function inside `.filter()` or `.map()` that always evaluates to the same value (like a memoized query parser or map lookup) performs unnecessary repeated work, adding overhead.
**Action:** Always evaluate static values *once* before loop operations, even if the value is memoized or cached, to avoid the function call and cache lookup overhead inside the loop.
