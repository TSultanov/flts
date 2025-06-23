import { PGlite } from '@electric-sql/pglite'
import { worker } from '@electric-sql/pglite/worker'
import { live } from '@electric-sql/pglite/live'

worker({
  async init() {
    // Create and return a PGlite instance
    return new PGlite('opfs-ahp://library-sql', { extensions: { live } })
  },
})