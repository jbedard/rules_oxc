import _defineProperty from "@oxc-project/runtime/helpers/defineProperty";
export class Fields {
	constructor(value) {
		_defineProperty(this, "declared", void 0);
		_defineProperty(this, "initialized", 1);
		this.declared = value;
	}
}
_defineProperty(Fields, "count", void 0);
_defineProperty(Fields, "label", "fields");
