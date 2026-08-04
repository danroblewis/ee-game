# The tutorials should be explored, not read

*Owner feedback, 2026-08-04, after playing the ten shipped intro rooms.
Recorded verbatim in substance; nothing here is built yet.*

## The verdict

> "The tutorials are fine, but they kind of suck."

The specific failure, and it is a damning one: **the owner already knows this
material, and still ended up reading the text and clicking only when
necessary.** A course that an expert skims is a course a beginner will skim
too — and a beginner who skims learns nothing, because the text was carrying
all the meaning.

## What is wrong

**It is a lot of reading.** The lesson card explains, and the circuit
illustrates. That is the wrong way round for this medium. We built a
simulator where every number is real and the picture moves; a wall of prose
in front of it is a textbook with a toy attached.

**Kirchhoff in particular is a visual law and is being taught as sentences.**
"Currents at a point sum to zero" is a thing you should SEE at a junction —
balls arriving and leaving, the count obviously conserved — not a sentence
you agree with and click past.

**The instruments are pre-placed, which spends the best thing in the game.**
The scopes and meters are already on the bench when the lesson opens. Those
are the *cool* things. A player should have to choose to place a probe, and
feel the small triumph of pointing it at the right node. Handing it to them
removes the only moment in the lesson where they act like an engineer.

## What it should be instead

**Exploration in service of education.** The room poses a situation; the
player pokes at it; understanding arrives from what they saw happen. The
check should confirm a discovery, not a chore.

**The intuition must come from the picture, not the paragraph.** Everything
needed is already built and already honest:

- the **schematic** itself, now that parts are rigid and rooms are laid out
- the **yellow current balls** — flow, direction, and rate, live
- the **voltage colouring** of wires — potential as a visible field
- **oscilloscopes and meters** the player places themselves

The text should still exist and should be *good* — explaining in detail what
each component is and how it works, for the player who wants it. But no
lesson should REQUIRE reading it to be passed or understood. Prose is the
appendix; the moving picture is the lesson.

**Sequence, as the owner put it:** current and flow first, then voltage
dividers, then everything else. Flow before pressure — which is the opposite
of how most courses order it, and is right for a simulator where flow is the
thing you can see.

## Implications for the shipped rooms

- Stop seeding probes and scopes in lesson templates; make placing one an
  early, celebrated step instead.
- Rebuild lesson 3 (Kirchhoff) around a junction the player watches, not a
  pair of sentences with a meter under them.
- Re-cut every lesson so the card is a prompt of one or two lines, with the
  detail available but out of the way.
- Re-order toward flow → divider → the rest.

---

# Two features the owner asked for

## 1 · A toggle for the current balls

A button to turn the yellow flow animation off and on. *"Sometimes it's nice
to have them off."*

Small, but note it is not merely cosmetic once the tutorials lean on the
balls for teaching: being able to remove them is also how a player checks
whether they actually understand the circuit without the hint. Client-side
display state; no reason for it to touch the document or the wire.

## 2 · A cursor-bound "scope it out" probe

Hold **shift** over a wire or a part and get, at the cursor:

- the voltage and the current as **numeric** values
- a **live oscilloscope trace**

Deliberately minimal — *"this oscilloscope doesn't need borders, and it
doesn't even need a background, or controls."* No chrome, no placement, no
persistence. A glance, not an instrument.

**Why it matters beyond convenience:** it is the lowest-friction way to look
at a signal that this game could possibly have, and it makes "check
everything" a habit rather than a chore. It also pairs exactly with the
tutorial direction above — a player who can interrogate any node in half a
second learns by poking, which is the whole point.

**Notes for whoever builds it:** the trace data already exists — probes
stream at 3.125 kHz and the client keeps 120 s per pid (`scope.ts`
`TraceStore`). The open question is what it shows for a node with no probe on
it, since the stream is per-probe: either a transient client-side capture, or
a reason to let the server sample a hovered node briefly. Worth deciding
before building.
