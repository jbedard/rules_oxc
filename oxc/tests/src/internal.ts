/** @internal */
export const secret: number = 1;

export const visible: number = 2;

/** @internal */
export function helper(): void {}

/** @internal */
export type Hidden = string;

/** @internal */
export class Private {}

export interface Options {
  /** @internal */
  debug?: boolean;
  name: string;
}

export class Api {
  /** @internal */
  hidden(): void {}

  /** @internal */
  state: number = 0;

  shown(): void {}
}
