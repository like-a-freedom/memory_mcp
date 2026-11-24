# Source-authority recall v4

This version corrects a temporal defect in v3: the literal path and URL events
used `t_ref=2026-11-22T12:00:00Z`, while primary recall was evaluated at the
process clock (about 06:16 UTC in the archived runs). Both references were
therefore future-dated and correctly invisible to ordinary recall. The v3
corpus and its reports remain unchanged as historical artifacts.

V4 preserves the same four probes, content, and oracle. It changes only the
reference events' `t_ref` to a fixed past timestamp so they are visible in
ordinary context regardless of the time of day. The ordinary fact remains an
independent positive control; the unknown endpoint remains an abstention
control. This is a corrected deterministic usability profile, not a claim that
v3 demonstrated a retrieval defect.
