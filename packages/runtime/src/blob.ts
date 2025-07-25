import * as tg from "./index.ts";
import { unindent } from "./template.ts";

export async function blob(
	strings: TemplateStringsArray,
	...placeholders: tg.Args<string>
): Promise<Blob>;
export async function blob(...args: tg.Args<Blob.Arg>): Promise<Blob>;
export async function blob(
	firstArg:
		| TemplateStringsArray
		| tg.Unresolved<tg.ValueOrMaybeMutationMap<Blob.Arg>>,
	...args: tg.Args<Blob.Arg>
): Promise<Blob> {
	return await inner(false, firstArg, ...args);
}

async function inner(
	raw: boolean,
	firstArg:
		| TemplateStringsArray
		| tg.Unresolved<tg.ValueOrMaybeMutationMap<Blob.Arg>>,
	...args: tg.Args<Blob.Arg>
): Promise<tg.Blob> {
	if (Array.isArray(firstArg) && "raw" in firstArg) {
		let strings = firstArg;
		let placeholders = args as tg.Args<string>;
		let components = [];
		for (let i = 0; i < strings.length - 1; i++) {
			let string = strings[i]!;
			components.push(string);
			let placeholder = placeholders[i]!;
			components.push(placeholder);
		}
		components.push(strings[strings.length - 1]!);
		let string = components.join("");
		if (!raw) {
			string = unindent([string]).join("");
		}
		return await Blob.new(string);
	} else {
		return await Blob.new(firstArg as tg.Blob.Arg, ...args);
	}
}

export class Blob {
	#state: Blob.State;

	constructor(state: Blob.State) {
		this.#state = state;
	}

	get state(): Blob.State {
		return this.#state;
	}

	static withId(id: Blob.Id): Blob {
		return new Blob({ id, stored: true });
	}

	static withObject(object: Blob.Object): Blob {
		return new Blob({ object, stored: false });
	}

	static fromData(data: Blob.Data): Blob {
		return Blob.withObject(Blob.Object.fromData(data));
	}

	static async new(...args: tg.Args<Blob.Arg>): Promise<Blob> {
		let arg = await Blob.arg(...args);
		let blob: Blob;
		if (!arg.children || arg.children.length === 0) {
			blob = Blob.withObject({ bytes: new Uint8Array() });
		} else if (arg.children.length === 1) {
			blob = arg.children[0]!.blob;
		} else {
			blob = Blob.withObject({ children: arg.children });
		}
		return blob;
	}

	static async leaf(
		...args: tg.Args<undefined | string | Uint8Array | tg.Blob>
	): Promise<Blob> {
		let resolved = await Promise.all(args.map(tg.resolve));
		let objects = await Promise.all(
			resolved.map(async (arg) => {
				if (arg === undefined) {
					return new Uint8Array();
				} else if (typeof arg === "string") {
					return tg.encoding.utf8.encode(arg);
				} else if (arg instanceof Uint8Array) {
					return arg;
				} else {
					return await arg.bytes();
				}
			}),
		);
		let length = objects.reduce(
			(length, bytes) => length + bytes.byteLength,
			0,
		);
		let bytes = new Uint8Array(length);
		let offset = 0;
		for (let entry of objects) {
			bytes.set(entry, offset);
			offset += entry.byteLength;
		}
		let object = { bytes };
		return Blob.withObject(object);
	}

	static async branch(...args: tg.Args<Blob.Arg>): Promise<Blob> {
		let arg = await Blob.arg(...args);
		return Blob.withObject({ children: arg.children ?? [] });
	}

	static async arg(...args: tg.Args<Blob.Arg>): Promise<Blob.ArgObject> {
		return await tg.Args.apply({
			args,
			map: async (arg) => {
				if (arg === undefined) {
					return { children: [] };
				} else if (typeof arg === "string") {
					let bytes = tg.encoding.utf8.encode(arg);
					let blob = Blob.withObject({ bytes });
					let length = bytes.length;
					return { children: [{ blob, length }] };
				} else if (arg instanceof Uint8Array) {
					let bytes = arg;
					let blob = Blob.withObject({ bytes });
					let length = bytes.length;
					return { children: [{ blob, length }] };
				} else if (arg instanceof Blob) {
					let length = await arg.length();
					let child = { blob: arg, length };
					return {
						children: [child],
					};
				} else {
					return arg;
				}
			},
			reduce: {
				children: "append",
			},
		});
	}

	static expect(value: unknown): Blob {
		tg.assert(value instanceof Blob);
		return value;
	}

	static assert(value: unknown): asserts value is Blob {
		tg.assert(value instanceof Blob);
	}

	get id(): Blob.Id {
		if (this.#state.id! !== undefined) {
			return this.#state.id;
		}
		let object = this.#state.object!;
		let data = Blob.Object.toData(object);
		let id = syscall("object_id", { kind: "blob", value: data });
		this.#state.id = id;
		return id;
	}

	async object(): Promise<Blob.Object> {
		await this.load();
		return this.#state.object!;
	}

	async load(): Promise<tg.Blob.Object> {
		if (this.#state.object === undefined) {
			let data = await syscall("object_get", this.#state.id!);
			tg.assert(data.kind === "blob");
			let object = Blob.Object.fromData(data.value);
			this.#state.object = object;
		}
		return this.#state.object!;
	}

	async store(): Promise<tg.Blob.Id> {
		await tg.Value.store(this);
		return this.id;
	}

	async children(): Promise<Array<tg.Object>> {
		let object = await this.object();
		return tg.Blob.Object.children(object);
	}

	async length(): Promise<number> {
		let object = await this.object();
		if ("children" in object) {
			return object.children
				.map(({ length }) => length)
				.reduce((a, b) => a + b, 0);
		} else {
			return object.bytes.byteLength;
		}
	}

	async read(arg?: Blob.ReadArg): Promise<Uint8Array> {
		let id = await this.store();
		return await syscall("blob_read", id, arg ?? {});
	}

	async bytes(): Promise<Uint8Array> {
		return await this.read();
	}

	async text(): Promise<string> {
		return tg.encoding.utf8.decode(await this.bytes());
	}
}

export namespace Blob {
	export type Arg = undefined | string | Uint8Array | tg.Blob | ArgObject;

	export type ArgObject = {
		children?: Array<Child> | undefined;
	};

	export type Id = string;

	export type State = tg.Object.State<tg.Blob.Id, tg.Blob.Object>;

	export type Object = Leaf | Branch;

	export type Leaf = {
		bytes: Uint8Array;
	};

	export type Branch = {
		children: Array<Child>;
	};

	export type Child = {
		blob: Blob;
		length: number;
	};

	export namespace Object {
		export let toData = (object: Object): Data => {
			if ("bytes" in object) {
				return {
					bytes: tg.encoding.base64.encode(object.bytes),
				};
			} else {
				return {
					children: object.children.map((child) => ({
						blob: child.blob.id,
						length: child.length,
					})),
				};
			}
		};

		export let fromData = (data: Data): Object => {
			if ("bytes" in data) {
				return {
					bytes: tg.encoding.base64.decode(data.bytes),
				};
			} else {
				return {
					children: data.children.map((child) => ({
						blob: tg.Blob.withId(child.blob),
						length: child.length,
					})),
				};
			}
		};

		export let children = (object: Object): Array<tg.Object> => {
			if ("children" in object) {
				return object.children.map(({ blob }) => blob);
			} else {
				return [];
			}
		};
	}

	export type Data = LeafData | BranchData;

	export type LeafData = {
		bytes: string;
	};

	export type BranchData = {
		children: Array<ChildData>;
	};

	export type ChildData = {
		blob: Blob.Id;
		length: number;
	};

	export type ReadArg = {
		position?: number | string | undefined;
		length?: number | undefined;
	};

	export let raw = async (
		strings: TemplateStringsArray,
		...placeholders: tg.Args<string>
	): Promise<Blob> => {
		return await inner(true, strings, ...placeholders);
	};
}
