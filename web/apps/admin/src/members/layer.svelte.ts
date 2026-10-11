import { getContext, setContext, type Snippet } from 'svelte';

const KEY = Symbol('members-dialogs');

/**
 * Where the Members page mounts the dialogs a sheet opens (a character's
 * Add/Edit and Delete): after the roster window, over the page as the boards
 * draw them, not inside the pane or the phone's member sheet that opened them.
 * One owner at a time: the open Characters tab.
 */
export class DialogLayer {
  content = $state<Snippet | null>(null);

  /** Show `dialogs` here while the caller is mounted (call from an `$effect`). */
  hold(dialogs: Snippet): () => void {
    this.content = dialogs;
    return () => {
      if (this.content === dialogs) this.content = null;
    };
  }
}

export const provideDialogLayer = (): DialogLayer => setContext(KEY, new DialogLayer());
export const dialogLayer = (): DialogLayer => getContext<DialogLayer>(KEY);
