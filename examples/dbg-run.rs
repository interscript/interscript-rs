use interscript_rs::interpreter::Runtime;
use interscript_rs::StageItem;

fn main() {
    let rt = Runtime::new(vec![
        "../maps/maps".into(),
        "../maps/libs".into(),
    ]);
    let doc = rt.load("alalc-ben-Beng-Latn-2017").unwrap();
    println!("deps declared: {:?}", doc.dep_aliases.keys().collect::<Vec<_>>());
    for (name, stage) in &doc.stages {
        for item in &stage.items {
            if let StageItem::Run { dep, stage, .. } = item {
                println!("stage {name}: run dep={dep:?} stage={stage:?}");
            }
        }
    }
    let un = rt.load("un-ben-Beng-Latn-2016").unwrap();
    let out = interscript_rs::interpreter::execute_stage_pub(&un, "main", "ত্য");
    println!("un-ben direct: {:?}", out);
}
