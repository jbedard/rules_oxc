/** @internal */
export declare const secret: number;
export declare const visible: number;
/** @internal */
export declare function helper(): void;
/** @internal */
export type Hidden = string;
/** @internal */
export declare class Private {}
export interface Options {
	/** @internal */
	debug?: boolean;
	name: string;
}
export declare class Api {
	/** @internal */
	hidden(): void;
	/** @internal */
	state: number;
	shown(): void;
}
