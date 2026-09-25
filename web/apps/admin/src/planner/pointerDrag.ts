/**
 * Pointer drag for the planner, loaded after first paint (idle or first
 * pointer over the board) so dnd-kit stays out of the initial bundle.
 * Keyboard moves live in keyboardMove.ts and never wait for this.
 *
 * dnd-kit's Feedback, Cursor and PreventSelection plugins inject a style
 * element (blocked by style-src 'self'); leaving them out keeps the
 * StyleInjector idle. Feedback also supplies the drag shape that collision
 * detection runs on, so it is set here: the source card at start, then the
 * pointer-following ghost, moved through CSSOM (not a style attribute).
 *
 * The whole card is the source, so activation always waits: a mouse or pen
 * must travel a few pixels (a click still opens the sheet) and touch needs a
 * long press (a tap opens it, a swipe still scrolls the board).
 */
import { AutoScroller, DragDropManager, Draggable, Droppable, PointerActivationConstraints, PointerSensor } from '@dnd-kit/dom';
import { DOMRectangle } from '@dnd-kit/dom/utilities';

export interface PointerDragEvents {
  start: (runId: string) => void;
  over: (day: number | null) => void;
  end: (runId: string, day: number | null, canceled: boolean) => void;
}

export interface PointerDrag {
  card: (card: HTMLElement, handle: HTMLElement | null, runId: string, movable: boolean) => () => void;
  day: (column: HTMLElement, day: number) => () => void;
  destroy: () => void;
}

function dayOf(id: unknown): number | null {
  const match = typeof id === 'string' ? /^day-(\d)$/.exec(id) : null;
  return match ? Number(match[1]) : null;
}

export function createPointerDrag(ghost: HTMLElement, on: PointerDragEvents): PointerDrag {
  const sensor = PointerSensor.configure({
    activationConstraints: (event) =>
      event.pointerType === 'touch'
        ? [new PointerActivationConstraints.Delay({ value: 250, tolerance: 5 })]
        : [new PointerActivationConstraints.Distance({ value: 6 })],
  });
  const manager = new DragDropManager({ plugins: [AutoScroller], sensors: [sensor], modifiers: [] });

  function placeGhost() {
    const { x, y } = manager.dragOperation.position.current;
    ghost.style.translate = `${Math.round(x + 14)}px ${Math.round(y + 14)}px`;
    if (getComputedStyle(ghost).display !== 'none') manager.dragOperation.shape = new DOMRectangle(ghost);
  }

  const off = [
    manager.monitor.addEventListener('dragstart', (event) => {
      const source = event.operation.source;
      if (source?.element) manager.dragOperation.shape = new DOMRectangle(source.element);
      on.start(String(source?.id ?? ''));
      placeGhost();
    }),
    manager.monitor.addEventListener('dragmove', () => placeGhost()),
    manager.monitor.addEventListener('dragover', (event) => on.over(dayOf(event.operation.target?.id))),
    manager.monitor.addEventListener('dragend', (event) =>
      on.end(String(event.operation.source?.id ?? ''), dayOf(event.operation.target?.id), event.canceled),
    ),
  ];

  return {
    card(card, handle, runId, movable) {
      const draggable = new Draggable({ id: runId, element: card, handle: handle ?? undefined, disabled: !movable }, manager);
      return () => draggable.destroy();
    },
    day(column, day) {
      const droppable = new Droppable({ id: `day-${day}`, element: column }, manager);
      return () => droppable.destroy();
    },
    destroy() {
      off.forEach((stop) => stop());
      manager.destroy();
    },
  };
}
