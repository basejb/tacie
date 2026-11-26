# Sample data (for onboarding)

A small made-up project, so that someone who has not connected any service yet can see answers appear during the
onboarding practice. It is not a real company or service.

- `repo/`: the sample project in Korean (a made-up payment webhook relay called "페이민트"). The sample index is built from it.
- `questions.json`: the practice questions and their answer files. The question vectors are built into the index at the
  same time.
- `index/`: the sample index built from those two. It goes into the app bundle as `Resources/Sample`.
- `en/`: the English version (the same made-up project, "PayMint", with the same facts and numbers): `en/repo/`,
  `en/questions.json` and `en/index/`, bundled as `Resources/Sample-en`. The app uses it when it runs in English
  (`SampleLanguage`).

Building the indexes (the command fails if any question's answer file is not among the top 3 chunks the search model
retrieves):

```bash
engine/target/release/tacie-engine index-folder sample/repo --out sample/index --questions sample/questions.json
engine/target/release/tacie-engine index-folder sample/en/repo --out sample/en/index --questions sample/en/questions.json
```

When you change the content, check that each practice question still has the right answer file. The Korean and English
versions must keep the same facts and numbers, so change both together.
