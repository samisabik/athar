// What the native app adds to the page's window.
interface Window {
  __TAURI_INTERNALS__: {
    invoke(cmd: string, args?: unknown, opts?: { headers?: Record<string, string> }): Promise<any>;
    transformCallback(cb: (payload: any) => void): number;
  };
  athar: Record<string, any>;
  atharConfirmQuit(): Promise<void>;
}

// Every element the interface builds or queries is an HTML element.
interface Element extends HTMLOrSVGElement, ElementCSSInlineStyle {
  hidden: boolean;
  disabled?: boolean;
  offsetParent: Element | null;
  click(): void;
  onclick: ((ev: MouseEvent) => any) | null;
  // Set on elements by the interface: the sound a pad or play button stands for, and how a
  // ruler shows a value changed from outside.
  padPath?: string;
  playPath?: string;
  sync?: () => void;
}
