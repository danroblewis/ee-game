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

---

# Ask questions. Do not give instructions.

*Owner, same session, and this is the strongest statement of the whole
direction:*

> "There should be questions for the player to answer by changing the
> schematic, there should not be instructions. When initially introducing
> topics, we can include diagrams and information explaining how electronics
> and electricity works, but we should favor having questions, and if we can
> have questions only then that is perfect. That is good learning."

## The distinction, concretely

Every step in the shipped course is an **instruction**. Lesson 1, step 1:

> "The bottom wire is missing. Press W, then drag across the gap. The lamp
> lights the moment the loop closes."

That sentence contains the observation, the diagnosis, the method AND the
expected result. Nothing is left for the player to find. Passing it requires
only obedience, and obedience teaches nothing — which is exactly why an
expert skimmed it and clicked.

The same room as a **question**:

> "The lamp is dark. Make it light."

Now the player has to look, notice the gap, know that a circuit must be a
loop, and find the tool. The check is unchanged. The room is unchanged. The
only thing removed is the answer — and removing the answer is the entire
lesson.

More of the same transformation:

| instruction (today) | question (wanted) |
|---|---|
| "Click any wire and press Delete. Everything stops at once." | "Stop the lamp WITHOUT touching it. What does that tell you about where current goes?" |
| "Turn the pot until the meter reads 1.00 V." | "Set this divider to hand you exactly 1.00 V. What did you have to know?" |
| "Wire the FET low-side; the source must sit at ground." | "Make the comparator switch this lamp. It will not work high-side — find out why." |

## The rule

**Information may be OFFERED; it may not be REQUIRED.** When a topic is
introduced for the first time, a diagram and a paragraph explaining how the
thing actually works are welcome — that is what a good textbook is for, and
this project cares about being right. But it belongs beside the question, not
in front of it, and a player who ignores it entirely must still be able to
reach the answer by experimenting on the schematic.

**The target is questions only.** Where a room can pose its problem with no
prose at all — a dark lamp, a crate on the floor, a meter reading the wrong
number — that is the best version of the room, and the prose that remains
should be there for the player who wants to know *why* after they have
already found out *that*.

## Why this compounds with everything else in this note

A question is unanswerable without observation, and observation is exactly
what the balls, the wire colouring, and a probe the player placed themselves
are for. Instructions make those decorations. Questions make them
instruments. The two halves of this document are the same change.
