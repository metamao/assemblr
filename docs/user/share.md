# Share your units

A unit is shared through its Git repository. Anyone who can read the
repository can use the unit, with the Git access they already have.

1. **Push the unit with its tags.** Create an empty repository on your Git
   host first, then connect it and push:

   ```sh
   git remote add origin https://github.com/acme/rust-service.git
   git push --follow-tags origin HEAD
   ```

   After each later `assemblr unit publish`, run the same `git push`. Push
   the units it builds on the same way.

2. **Hand out the URL.** Others register the unit on their machine:

   ```sh
   assemblr registry add https://github.com/acme/rust-service.git
   ```

   Registering a unit does not register the units it builds on. They must be
   registered too.

To use the units in a project, see [Use units in a project](use.md).
