{-
messages/acting.dhall — every operational script in this repository (`*.sh`,
`*.py`), classified, with a ceiling per class; a test needs no row when its path
says it is one. What the classes mean and when a ceiling may rise:
dev-lint/tables/acting_schema.dhall. The gate runs `dev-lint-fleet --acting .`:
a script with no row, a row for a file that is gone, or a class over its
ceiling fails the commit that causes it.
-}
let A = ../dev-lint/tables/acting_schema.dhall

let subject = A.subject

let Class = A.Class

in  { ceilings =
        -- acts: each row names what it becomes (automation-model items 13-15)
        -- bootstrap: one-off imports and enrolments
      { acts = 2
      , bootstrap = 2
      , by_design = 0
      , observes = 0
      , observer_checks = 0
      }
    , subjects =
      [ subject
              "android/deploy.sh"
              Class.Acts
              "builds the APK and installs it on a phone over adb: the phone-shells plan's target (automation-model item 14)"
          , subject "archiver/tools/check_known_truths.py" Class.OutOfScope ""
          , subject
              "archiver/tools/import_gchat.py"
              Class.Acts
              "writes the Google Chat archive into the signal database"
          , subject
              "archiver/tools/import_jsonl.py"
              Class.Bootstrap
              "imports one Signal export into the database, once per export"
          , subject
              "archiver/tools/reconcile_groups.py"
              Class.Bootstrap
              "a one-time merge of group threads keyed by masterKey"
          , subject "scripts/gen-types.sh" Class.OutOfScope ""
          , subject "scripts/setup-hooks.sh" Class.OutOfScope ""
      ]
    }
