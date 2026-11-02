import { checkDocumentation, wikiPages } from '../../scripts/documentation.mjs';

const result = await checkDocumentation();
await wikiPages();
console.log(`Documentation: ${result.links} local links checked in ${result.pages} pages; Wiki navigation valid.`);
