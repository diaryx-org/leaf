# A face for the tests

`leaf-test-face.ttf` is Atkinson Hyperlegible Next (SIL Open Font License,
`OFL.txt`), cut down to the Latin letters and a space, fixed at weight 400,
and renamed `Leaf Test Face` so that no system has a family by its name.
`text.rs`'s tests bring it as a host's [`Faces`] and check that parley sets
text in it.
