/** Types for legacy Agents View daemon catalogs. Devo product uses Native backends. */

export type DaemonCommandBody = { type: string; [key: string]: unknown };
export type DaemonHello = { type: "daemon_hello"; [key: string]: unknown };
export type DaemonClientMessageListener = (message: unknown) => void;
export type DaemonClientCloseListener = (error: Error) => void;
export type DaemonClientProgressListener = (message: unknown) => void;
export interface DaemonClientRequestOptions {
	[key: string]: unknown;
}
export class DaemonSocketClosedError extends Error {}
export class DaemonCapabilityUnavailableError extends Error {}
export function getDaemonSocketCloseReason(_error: Error): string | undefined {
	return undefined;
}
export type DaemonClientReconnectStatus = string;
export interface DaemonClientReconnectOptions {
	[key: string]: unknown;
}

/** Minimal client surface used by optional daemon roster/catalog helpers. */
export interface DaemonTransportClient {
	request(command: unknown, options?: unknown): Promise<unknown>;
	onMessage?(listener: DaemonClientMessageListener): void;
	close?(): void;
	readonly isConnected?: boolean;
	readonly hello?: DaemonHello;
	waitForHello?(): Promise<DaemonHello | undefined>;
	supportsServerCapability?(name: string): boolean;
}
