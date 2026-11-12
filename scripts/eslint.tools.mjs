import jsdoc from "eslint-plugin-jsdoc";
import project from "../tsconfig.tools.json" with { type: "json" };

export default [
	{
		files: project.files,
		linterOptions: {
			noInlineConfig: true,
			reportUnusedDisableDirectives: "error",
		},
		plugins: { jsdoc },
		rules: {
			"jsdoc/require-jsdoc": [
				"error",
				{ require: { FunctionDeclaration: true } },
			],
			"jsdoc/require-param": "error",
			"jsdoc/require-param-type": "error",
			"jsdoc/require-returns": ["error", { forceRequireReturn: true }],
			"jsdoc/require-returns-type": "error",
			"jsdoc/reject-any-type": "error",
			"no-warning-comments": [
				"error",
				{
					terms: ["ts-ignore", "ts-nocheck", "ts-expect-error"],
					location: "anywhere",
				},
			],
		},
	},
];
