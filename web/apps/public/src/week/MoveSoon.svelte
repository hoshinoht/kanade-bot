<!--
  "Move this run (this week only)" as board Week-RunMine draws it, disabled
  and marked "coming soon" (moves arrive with member writes): the admin Move
  picker's look (`_move-picker.scss`, ui `DayStrip`), the run's own day and
  time chosen, every control off.
-->
<script lang="ts">
  import type { MemberRun, MemberWeek } from '@kanade/api-types';
  import { dayLabel, dayNumber, DayStrip } from '@kanade/ui';

  let { run, week }: { run: MemberRun; week: MemberWeek } = $props();

  const today = $derived(week.days.find((d) => d.is_today)?.index ?? -1);
  const days = $derived(
    week.days.map((d) => {
      const others = week.runs.filter((r) => r.day === d.index && r.id !== run.id).length;
      return {
        value: d.index,
        dow: d.dow,
        date: dayNumber(d.date),
        tag: d.is_reset ? 'reset' : d.is_today ? 'today' : undefined,
        dots: Math.min(3, others),
        past: d.index < today,
        today: d.is_today,
        reset: d.is_reset,
        from: d.index === run.day,
        label: `${dayLabel(week, d.index)}${d.index === run.day ? ', this run' : ''}${others ? `, ${others} other runs` : ''}`,
      };
    }),
  );
  const when = $derived(`${dayLabel(week, run.day)} ${run.time ?? 'own time'}`);
</script>

<section class="movepick movepick--pane member-move" aria-labelledby="member-move-title" data-fid="week-move">
  <fieldset class="movepick__set" disabled>
    <legend class="movepick__legend">
      <span class="cap movepick__title" id="member-move-title">Move this run (this week only)</span>
      <span class="status-chip status-chip--warn">coming soon</span>
    </legend>
    <DayStrip {days} value={run.day} label="Day" onpick={() => {}} />
    <div class="movepick__row" data-fid="move-time">
      <div class="timestep timestep--off">
        <span class="timestep__btn" aria-hidden="true"
          ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
            ><path d="M6 9l6 6 6-6" /></svg
          ></span
        >
        <span class="timestep__value mono">{run.time ?? '--:--'}</span>
        <span class="timestep__btn" aria-hidden="true"
          ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
            ><path d="M6 15l6-6 6 6" /></svg
          ></span
        >
      </div>
      <span class="movepick__or" aria-hidden="true">or</span>
      <label class="movepick__typed" data-fid="move-type">
        <input class="mono" aria-label="Type a day and time" placeholder="wed 21:30, 9:45pm" />
      </label>
    </div>
    <p class="movepick__hint">Your weekly timing stays the same. Admins can undo moves.</p>
  </fieldset>
  <div class="movepick__foot">
    <p class="movepick__result" data-fid="move-result">
      <span class="cap">Moves to</span>
      <b class="mono movepick__to">{when}</b>
    </p>
    <div class="movepick__acts" data-fid="move-submit">
      <button type="button" class="btn" aria-disabled="true">Cancel</button>
      <button type="button" class="btn btn--primary btn--key movepick__go" aria-disabled="true">Move…</button>
    </div>
  </div>
</section>
