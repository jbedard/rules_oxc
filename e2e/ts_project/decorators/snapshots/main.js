import _decorateMetadata from "@oxc-project/runtime/helpers/decorateMetadata";
import _decorate from "@oxc-project/runtime/helpers/decorate";
function dec(_target, _key) {}
export class Service {
	name = "svc";
	describe(label) {
		return label ? label.length : 0;
	}
}
_decorate([dec, _decorateMetadata("design:type", String)], Service.prototype, "name", void 0);
_decorate([
	dec,
	_decorateMetadata("design:type", Function),
	_decorateMetadata("design:paramtypes", [Object]),
	_decorateMetadata("design:returntype", Number)
], Service.prototype, "describe", null);
