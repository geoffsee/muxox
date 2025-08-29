let counter = 0;

setInterval(() => {
    console.log(`Hello #${counter} from service 1`);
    counter += 1;
}, 333);