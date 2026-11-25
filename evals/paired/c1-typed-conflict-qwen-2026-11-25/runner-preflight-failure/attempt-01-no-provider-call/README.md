# Runner preflight failure — attempt 01

The paired runner stopped while validating the independent public MCP probe.
The registered plan expected MCP schema SHA-256
`9475b795bde1360eda3cf672b8c75d99ff1fa6cd0e91b219907a7fc81c71880f`; the
current product binary produced
`cc5339eb0b61c4a1ab93caf7f48b6e9d9c0b1c33a8fe5deb72658fe1abfdb752` on all
four deterministic preflight captures.

The runner calls and validates the independent probe before the model backend
executes each task. Therefore this attempt made no model task requests and
produced no trial receipts. The two zero-byte receipt files are preserved as
evidence that output initialization began. No score from this attempt enters a
C1 denominator.

The corrected pre-registration pins the observed schema digest and has its own
freeze manifest. This failed preflight remains excluded from the cohort.
