import _decorate from "custom-helpers/helpers/decorate";
function dec(_target, _key) {}
export class Service {
	name = "svc";
	describe(label) {
		return label ? label.length : 0;
	}
}
_decorate([dec], Service.prototype, "name", void 0);
_decorate([dec], Service.prototype, "describe", null);
